//! Offers the LosOS boxes on the local network as applications.
//!
//!     losos-boxes watch           keep an entry per box in the app list
//!     losos-boxes list            print the boxes found in a few seconds
//!     losos-boxes clear           remove the entries `watch` wrote
//!     losos-boxes terminal <url>  sign in to a box when needed, then open it
//!
//! `watch` is a user service (nixos/modules/losos-boxes.nix). It browses
//! `_losos._tcp` and `_workstation._tcp` through systemd-resolved, which
//! answers mDNS on this OS, and writes a `.desktop` file for each box into
//! the user's applications directory, which derisk's palette and app list
//! read each time the palette opens. `terminal` is the command those entries'
//! "Manage in the terminal" action runs in derisk-terminal.

use std::io::{self, BufRead, BufReader, IsTerminal, Read, Write};
use std::net::{IpAddr, Shutdown, SocketAddr, SocketAddrV6, TcpStream};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use miette::{bail, miette, IntoDiagnostic, Result, WrapErr};
use serde_json::{json, Value};
use tracing::{debug, info, warn};
use tracing_subscriber::EnvFilter;

use losos_boxes::{
    browse_events, frame, is_box_state, ok_body, parse_reply, parse_resolved, resolve_parameters,
    state_request, Boxes, Change, Instance, LososBox, Seen, BOX_TYPE, DOMAIN, ENTRY_PREFIX,
    RESOLVE_SOCKET, WORKSTATION_TYPE,
};

/// How long a workstation gets to answer for `/setup/state.json`, per
/// address. A box on the LAN answers in milliseconds.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long `list` browses: mDNS answers a query within a second, and
/// resolved asks again shortly after.
const LIST_FOR: Duration = Duration::from_secs(4);

fn main() -> ExitCode {
    // stderr, which is the journal for a service.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(io::stderr)
        .with_ansi(io::stderr().is_terminal())
        .without_time()
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("watch") => entries_dir().and_then(|dir| watch(&dir)),
        Some("list") => list(),
        Some("clear") => entries_dir().and_then(|dir| clear(&dir)),
        Some("terminal") => match args.get(1) {
            Some(url) => return terminal(url),
            None => Err(miette!("usage: losos-boxes terminal <url>")),
        },
        _ => Err(miette!(
            "usage: losos-boxes watch | list | clear | terminal <url>"
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            eprintln!("{report:?}");
            ExitCode::FAILURE
        }
    }
}

/// `$XDG_DATA_HOME/applications`, which derisk reads before the system's.
fn entries_dir() -> Result<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .ok_or_else(|| miette!("neither XDG_DATA_HOME nor HOME is set"))?;
    Ok(data.join("applications"))
}

/// A connection to resolved's Varlink socket.
struct Varlink {
    reader: BufReader<UnixStream>,
}

impl Varlink {
    fn connect() -> Result<Varlink> {
        let stream = UnixStream::connect(RESOLVE_SOCKET)
            .into_diagnostic()
            .wrap_err_with(|| format!("cannot reach systemd-resolved at {RESOLVE_SOCKET}"))?;
        Ok(Varlink {
            reader: BufReader::new(stream),
        })
    }

    /// The stream under the reader, for shutting it down from another thread.
    fn handle(&self) -> Result<UnixStream> {
        self.reader.get_ref().try_clone().into_diagnostic()
    }

    fn send(&mut self, method: &str, parameters: Value, more: bool) -> Result<()> {
        self.reader
            .get_mut()
            .write_all(&frame(method, parameters, more))
            .into_diagnostic()
    }

    /// The next reply, or `None` when resolved hung up.
    fn receive(&mut self) -> Result<Option<losos_boxes::Reply>> {
        let mut message = Vec::new();
        let read = self.reader.read_until(0, &mut message).into_diagnostic()?;
        if read == 0 || message.last() != Some(&0) {
            return Ok(None);
        }
        message.pop();
        parse_reply(&message).map(Some).map_err(|e| miette!("{e}"))
    }

    /// One call and its one reply.
    fn call(&mut self, method: &str, parameters: Value) -> Result<Value> {
        self.send(method, parameters, false)?;
        match self.receive()? {
            Some(reply) => Ok(reply.parameters),
            None => bail!("systemd-resolved hung up during {method}"),
        }
    }
}

/// Browses `service_type` until resolved hangs up or the connection is shut
/// down, sending each instance it adds or removes. Returns why it stopped.
fn browse(
    mut varlink: Varlink,
    service_type: &str,
    seen: crossbeam_channel::Sender<Seen>,
) -> Result<()> {
    varlink.send(
        "io.systemd.Resolve.BrowseServices",
        json!({ "type": service_type, "domain": DOMAIN }),
        true,
    )?;
    while let Some(reply) = varlink.receive()? {
        for event in browse_events(&reply.parameters) {
            if seen.send(event).is_err() {
                return Ok(());
            }
        }
        if !reply.continues {
            break;
        }
    }
    bail!("systemd-resolved stopped browsing {service_type}")
}

/// What `instance` is, if it is a box: resolved for its host, and, for a
/// workstation, asked for a box's state.
fn identify(resolver: &mut Varlink, instance: &Instance) -> Result<Option<LososBox>> {
    let reply = resolver.call(
        "io.systemd.Resolve.ResolveService",
        resolve_parameters(instance),
    )?;
    let Some(resolved) = parse_resolved(&reply) else {
        return Ok(None);
    };
    if instance.service_type == BOX_TYPE {
        return Ok(LososBox::announced(&resolved));
    }
    // A workstation: a box serves its state on its front port, 80, whatever
    // port the workstation record names.
    Ok(LososBox::new(&resolved.host, 80, None).filter(|b| probe(&b.host, &resolved.addresses)))
}

/// Whether the machine at `addresses` answers `/setup/state.json` as a box.
/// IPv4 first: a box's nginx listens there on every version so far.
fn probe(host: &str, addresses: &[(IpAddr, u32)]) -> bool {
    let mut ordered: Vec<&(IpAddr, u32)> = addresses.iter().collect();
    ordered.sort_by_key(|(ip, _)| ip.is_ipv6());
    ordered.into_iter().any(|&(ip, ifindex)| {
        let socket = match ip {
            IpAddr::V4(_) => SocketAddr::new(ip, 80),
            IpAddr::V6(v6) => {
                // A link-local address means nothing without its interface.
                let scope = if v6.segments()[0] & 0xffc0 == 0xfe80 {
                    ifindex
                } else {
                    0
                };
                SocketAddr::V6(SocketAddrV6::new(v6, 80, 0, scope))
            }
        };
        let answer = (|| -> io::Result<Vec<u8>> {
            let mut stream = TcpStream::connect_timeout(&socket, PROBE_TIMEOUT)?;
            stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
            stream.set_write_timeout(Some(PROBE_TIMEOUT))?;
            stream.write_all(state_request(host).as_bytes())?;
            let mut response = Vec::new();
            stream.take(64 * 1024).read_to_end(&mut response)?;
            Ok(response)
        })();
        match answer {
            Ok(response) => ok_body(&response).is_some_and(is_box_state),
            Err(e) => {
                debug!(%host, %socket, error = %e, "no answer for /setup/state.json");
                false
            }
        }
    })
}

/// Browses both types and hands every change in the boxes present to
/// `apply`, until resolved stops browsing or `until` passes. Two threads
/// read the browsers; this one resolves and probes, one instance at a time,
/// so the order of a removal after its addition is kept.
fn run(until: Option<Instant>, mut apply: impl FnMut(&Change) -> Result<()>) -> Result<Boxes> {
    let browsers = [BOX_TYPE, WORKSTATION_TYPE]
        .into_iter()
        .map(|t| Ok((t, Varlink::connect()?)))
        .collect::<Result<Vec<_>>>()?;
    let handles = browsers
        .iter()
        .map(|(_, v)| v.handle())
        .collect::<Result<Vec<_>>>()?;
    let mut resolver = Varlink::connect()?;
    let mut boxes = Boxes::default();

    let (seen_tx, seen) = crossbeam_channel::bounded::<Seen>(64);
    let (done_tx, done) = crossbeam_channel::bounded::<Result<()>>(browsers.len());
    std::thread::scope(|scope| {
        for (service_type, varlink) in browsers {
            let (seen_tx, done_tx) = (seen_tx.clone(), done_tx.clone());
            scope.spawn(move || {
                let _ = done_tx.send(browse(varlink, service_type, seen_tx));
            });
        }
        drop(seen_tx);

        let deadline = match until {
            Some(until) => crossbeam_channel::at(until),
            None => crossbeam_channel::never(),
        };
        let outcome = (|| -> Result<()> {
            loop {
                let event = crossbeam_channel::select! {
                    recv(seen) -> event => match event {
                        Ok(event) => event,
                        // Both browsers stopped; the first reason is the one to report.
                        Err(_) => return done.try_recv().unwrap_or(Ok(())),
                    },
                    // One browser stopping is the end of watching: a box
                    // could no longer leave the list, or join it.
                    recv(done) -> why => return why.unwrap_or(Ok(())),
                    recv(deadline) -> _ => return Ok(()),
                };
                let change = match event {
                    Seen::Added(instance) => match identify(&mut resolver, &instance) {
                        Ok(Some(found)) => boxes.add(instance, found),
                        Ok(None) => None,
                        Err(e) => {
                            debug!(name = %instance.name, error = ?e, "cannot resolve");
                            None
                        }
                    },
                    Seen::Removed(instance) => boxes.remove(&instance),
                };
                if let Some(change) = change {
                    apply(&change)?;
                }
            }
        })();

        // Wakes the browsers from their reads, so the scope can end.
        for handle in &handles {
            let _ = handle.shutdown(Shutdown::Both);
        }
        outcome
    })?;
    Ok(boxes)
}

/// Keeps one entry per box present, until resolved stops browsing, which
/// the service restarts after. Entries from an earlier run go first: their
/// boxes may have left while nothing watched.
fn watch(dir: &Path) -> Result<()> {
    clear(dir)?;
    std::fs::create_dir_all(dir)
        .into_diagnostic()
        .wrap_err_with(|| format!("cannot create {}", dir.display()))?;
    info!(dir = %dir.display(), "watching for LosOS boxes");
    run(None, |change| match change {
        Change::Write(found) => {
            info!(host = %found.host, version = ?found.version, "box present");
            let path = dir.join(found.file_name());
            let partial = dir.join(format!(".{}.partial", found.file_name()));
            std::fs::write(&partial, found.desktop_entry())
                .and_then(|()| std::fs::rename(&partial, &path))
                .into_diagnostic()
                .wrap_err_with(|| format!("cannot write {}", path.display()))
        }
        Change::Remove(file) => {
            info!(%file, "box gone");
            remove(&dir.join(file))
        }
    })?;
    Ok(())
}

/// Prints the boxes found within a few seconds, one per line.
fn list() -> Result<()> {
    let boxes = run(Some(Instant::now() + LIST_FOR), |_| Ok(()))?;
    let mut out = io::stdout().lock();
    for found in boxes.iter() {
        let version = found.version.as_deref().unwrap_or("");
        writeln!(out, "{}\t{}\t{version}", found.name(), found.url()).into_diagnostic()?;
    }
    Ok(())
}

/// Removes every entry `watch` wrote.
fn clear(dir: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).into_diagnostic(),
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(ENTRY_PREFIX) && name.ends_with(".desktop") {
            remove(&entry.path())?;
        }
    }
    Ok(())
}

fn remove(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e)
            .into_diagnostic()
            .wrap_err_with(|| format!("cannot remove {}", path.display())),
        _ => Ok(()),
    }
}

/// Opens `url` in `losos`, signing in first when `losos hosts` does not list
/// it. Runs in a terminal window of its own, which closes when this returns,
/// so a failure waits for Enter: `losos` has said what went wrong, and the
/// window should stay until it has been read.
fn terminal(url: &str) -> ExitCode {
    let result = (|| -> Result<()> {
        let hosts = Command::new("losos")
            .arg("hosts")
            .output()
            .into_diagnostic()
            .wrap_err("cannot run losos")?;
        let listed = String::from_utf8_lossy(&hosts.stdout);
        if !losos_boxes::logged_in_hosts(&listed).any(|h| h == url) {
            run_losos(&["login", url])?;
        }
        run_losos(&["open", url])
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(report) => {
            warn!(%url, "{report}");
            eprint!("\nPress Enter to close this window.");
            let _ = io::stdin().lock().read_line(&mut String::new());
            ExitCode::FAILURE
        }
    }
}

fn run_losos(args: &[&str]) -> Result<()> {
    let status = Command::new("losos")
        .args(args)
        .status()
        .into_diagnostic()
        .wrap_err("cannot run losos")?;
    if !status.success() {
        bail!("losos {} did not finish", args[0]);
    }
    Ok(())
}
