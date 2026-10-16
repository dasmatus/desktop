//! Downloads through aria2 (docs/danube.md, "Downloads"): WebKit finds out
//! that a response is a file to save, and aria2 fetches it, over as many
//! connections as the computer has hardware threads.
//!
//! WebKit's own downloader fetches a file over the one connection its
//! response arrived on. aria2 splits a file into ranges and fetches them
//! at once, which is what makes a large file from a server that caps each
//! connection's speed arrive in a fraction of the time. Patching WebKit's
//! network process to do that would mean carrying a download manager in
//! C++ against every WebKit release; handing the transfer over once WebKit
//! has the response (engine.rs, `watch_download`) keeps WebKit unpatched
//! and still decides everything a browser must: which responses are
//! downloads, the file's name, the cookies, Referer and User-Agent sent.
//!
//! One aria2c runs per browser, started with the first download and
//! controlled over its JSON-RPC interface on a loopback port, with a
//! random secret no other process knows. It runs inside Danube's sandbox,
//! so it can reach the network and write to Downloads and nothing else in
//! the home directory, and it stops when Danube does (`--stop-with-process`).
//! This thread owns it: WebKit's thread sends it [`Request`]s, and it
//! answers with [`Update`]s through WebKit's command channel, polling
//! aria2 twice a second while anything is downloading.

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use serde_json::{json, Value};
use tracing::{debug, info, warn};

use crate::shared::{Command as EngineCommand, Commands, DownloadCommand};

/// A download, as long as Danube runs. Never reused.
pub type DownloadId = u64;

/// aria2 refuses more connections than this to one server for one
/// download (`--max-connection-per-server` takes 1 to 16).
const MAX_PER_SERVER: usize = 16;

/// How often aria2 is asked how its downloads are doing.
const POLL: Duration = Duration::from_millis(500);

/// A file for aria2 to fetch, with what WebKit would have sent for it.
#[derive(Clone, Debug, PartialEq)]
pub struct Job {
    pub id: DownloadId,
    /// The address WebKit's response came from, after any redirects.
    pub uri: String,
    /// The file, already created empty so no other download takes its name.
    pub path: PathBuf,
    /// The `Cookie` header WebKit's cookie jar has for `uri`.
    pub cookie: Option<String>,
    pub referer: Option<String>,
    pub user_agent: String,
}

/// What WebKit's thread asks of this one.
#[derive(Debug)]
pub enum Request {
    Start(Job),
    Cancel(DownloadId),
}

/// How a download is doing, or how it ended.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    Progress {
        id: DownloadId,
        received: u64,
        /// `None` while the size is not known, or when the server never
        /// says it.
        total: Option<u64>,
        /// Bytes a second, over aria2's last few seconds.
        speed: u64,
        connections: u32,
    },
    Done(DownloadId),
    /// Ended without the file. With `retry`, nothing of it had arrived,
    /// and WebKit's thread lets WebKit fetch it instead: aria2 could not
    /// start, or the server wants something aria2 does not have, such as
    /// a login only the page's session holds.
    Failed {
        id: DownloadId,
        retry: bool,
        /// aria2 was running; `false` when it could not be started.
        started: bool,
        message: String,
    },
    Cancelled(DownloadId),
}

/// How many connections a download is split over: one per hardware
/// thread, unless the settings say otherwise, as split and as connections
/// to one server (which aria2 caps).
pub fn connections(setting: Option<usize>) -> (usize, usize) {
    let threads = setting.unwrap_or_else(|| {
        std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
    });
    let split = threads.max(1);
    (split, split.min(MAX_PER_SERVER))
}

/// Runs until WebKit's thread drops its end of `requests`, which it does
/// when it stops; what is still downloading then is removed with aria2.
pub fn run(requests: Receiver<Request>, commands: &Commands, split: (usize, usize)) {
    let tell = |update: Update| commands.send(EngineCommand::Download(DownloadCommand::Update(update)));
    let mut aria2: Option<Aria2> = None;
    // Each running download's gid and file.
    let mut active: HashMap<DownloadId, (String, PathBuf)> = HashMap::new();
    // Cancelled while aria2 still had it: removed once aria2 says so.
    let mut cancelling: HashSet<DownloadId> = HashSet::new();
    // Cancelled before its Start arrived (WebKit's thread was still
    // fetching its cookies).
    let mut cancelled_early: HashSet<DownloadId> = HashSet::new();
    let mut polled = Instant::now();
    loop {
        let request = if active.is_empty() {
            requests.recv().map_err(|_| RecvTimeoutError::Disconnected)
        } else {
            requests.recv_timeout(POLL.saturating_sub(polled.elapsed()))
        };
        match request {
            Ok(Request::Start(job)) => {
                if cancelled_early.remove(&job.id) {
                    remove_partial(&job.path);
                    tell(Update::Cancelled(job.id));
                    continue;
                }
                if aria2.is_none() {
                    match Aria2::start(split) {
                        Ok(started) => aria2 = Some(started),
                        Err(error) => {
                            warn!(%error, "cannot start aria2; WebKit downloads instead");
                            remove_partial(&job.path);
                            tell(Update::Failed {
                                id: job.id,
                                retry: true,
                                started: false,
                                message: error,
                            });
                            continue;
                        }
                    }
                }
                let rpc = aria2.as_ref().expect("started above");
                match rpc.call("aria2.addUri", vec![json!([job.uri]), add_options(&job)]) {
                    Ok(Value::String(gid)) => {
                        info!(id = job.id, gid, "aria2 is downloading");
                        active.insert(job.id, (gid, job.path));
                    }
                    other => {
                        let message = other.err().unwrap_or_else(|| "aria2 gave no gid".into());
                        warn!(id = job.id, message, "aria2 refused a download");
                        remove_partial(&job.path);
                        tell(Update::Failed {
                            id: job.id,
                            retry: true,
                            started: true,
                            message,
                        });
                    }
                }
            }
            Ok(Request::Cancel(id)) => match (active.get(&id), &aria2) {
                (Some((gid, _)), Some(rpc)) => {
                    // forceRemove does not wait for servers to answer a
                    // polite close; the files go once aria2 lets go of them.
                    if let Err(error) = rpc.call("aria2.forceRemove", vec![json!(gid)]) {
                        debug!(id, error, "aria2 had already finished it");
                    }
                    cancelling.insert(id);
                }
                _ => {
                    cancelled_early.insert(id);
                }
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if active.is_empty() || polled.elapsed() < POLL {
            continue;
        }
        polled = Instant::now();
        let Some(rpc) = &aria2 else { continue };
        let ids: Vec<DownloadId> = active.keys().copied().collect();
        let calls = ids
            .iter()
            .map(|id| {
                json!({
                    "methodName": "aria2.tellStatus",
                    "params": [rpc.token(), active[id].0, STATUS_KEYS],
                })
            })
            .collect::<Vec<_>>();
        let answers = match rpc.call_bare("system.multicall", json!([calls])) {
            Ok(Value::Array(answers)) => answers,
            Ok(_) | Err(_) => {
                // aria2 died under us: what it had is fetched again by
                // WebKit, from the start, since its partial file is gone.
                warn!("aria2 stopped answering");
                for (id, (_, path)) in active.drain() {
                    remove_partial(&path);
                    tell(if cancelling.remove(&id) {
                        Update::Cancelled(id)
                    } else {
                        Update::Failed {
                            id,
                            retry: true,
                            started: true,
                            message: "aria2 stopped".into(),
                        }
                    });
                }
                aria2 = None;
                continue;
            }
        };
        for (id, answer) in ids.into_iter().zip(answers) {
            // multicall wraps each result in a one-element array.
            let status = match answer {
                Value::Array(mut one) if !one.is_empty() => Status::read(id, &one.remove(0)),
                other => Status::read(id, &other),
            };
            let Some(status) = status else { continue };
            let finished = !matches!(status, Update::Progress { .. });
            if finished {
                let (gid, path) = active.remove(&id).expect("polled ids are active");
                let _ = rpc.call("aria2.removeDownloadResult", vec![json!(gid)]);
                let cancelled = cancelling.remove(&id);
                if cancelled || matches!(status, Update::Failed { .. }) {
                    remove_partial(&path);
                }
                tell(if cancelled { Update::Cancelled(id) } else { status });
            } else if !cancelling.contains(&id) {
                tell(status);
            }
        }
    }
    // WebKit stopped: what has not finished cannot be resumed from here
    // (docs/not-done.md), so its partial files go with aria2.
    drop(aria2);
    for (_, path) in active.into_values() {
        remove_partial(&path);
    }
}

/// What aria2 is asked about each download.
const STATUS_KEYS: [&str; 7] = [
    "status",
    "totalLength",
    "completedLength",
    "downloadSpeed",
    "connections",
    "errorCode",
    "errorMessage",
];

struct Status;

impl Status {
    /// aria2's `tellStatus` answer as an update; `None` while it is only
    /// waiting in aria2's queue or paused, which says nothing new.
    fn read(id: DownloadId, status: &Value) -> Option<Update> {
        // aria2 writes every number as a decimal string.
        let number = |key: &str| status[key].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        let received = number("completedLength");
        match status["status"].as_str()? {
            "active" => Some(Update::Progress {
                id,
                received,
                total: Some(number("totalLength")).filter(|&t| t > 0),
                speed: number("downloadSpeed"),
                connections: number("connections") as u32,
            }),
            "complete" => Some(Update::Done(id)),
            "removed" => Some(Update::Cancelled(id)),
            "error" => Some(Update::Failed {
                id,
                retry: received == 0,
                started: true,
                message: status["errorMessage"]
                    .as_str()
                    .filter(|m| !m.is_empty())
                    .map_or_else(|| format!("aria2 error {}", number("errorCode")), str::to_owned),
            }),
            _ => None,
        }
    }
}

/// The options one download is added with: where it goes and what is
/// sent for it.
fn add_options(job: &Job) -> Value {
    let dir = job.path.parent().unwrap_or(Path::new("."));
    let name = job.path.file_name().unwrap_or_default();
    let mut options = json!({
        "dir": dir.to_string_lossy(),
        "out": name.to_string_lossy(),
        // The empty file reserving the name is ours to write over.
        "allow-overwrite": "true",
        "user-agent": job.user_agent,
    });
    // The Cookie header goes to `uri`'s host only: WebKit already
    // followed the redirects, so aria2 asks for the final address.
    if let Some(cookie) = &job.cookie {
        options["header"] = json!([format!("Cookie: {cookie}")]);
    }
    if let Some(referer) = &job.referer {
        options["referer"] = json!(referer);
    }
    options
}

/// Removes a download's file and aria2's control file beside it.
fn remove_partial(path: &Path) {
    let _ = std::fs::remove_file(path);
    let mut control = path.as_os_str().to_owned();
    control.push(".aria2");
    let _ = std::fs::remove_file(control);
}

/// The aria2c this thread started, and how to talk to it.
struct Aria2 {
    child: Child,
    port: u16,
    secret: String,
}

impl Aria2 {
    /// Starts aria2c with `split` connections a download, `split.1` of
    /// them to any one server, and waits for its RPC port to answer.
    fn start(split: (usize, usize)) -> Result<Self, String> {
        let secret = secret().map_err(|e| format!("no randomness for aria2's secret: {e}"))?;
        // A port the kernel has just handed out, given to aria2; another
        // program taking it in between only makes aria2 fail to start.
        let port = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .and_then(|l| l.local_addr())
            .map_err(|e| format!("no loopback port for aria2: {e}"))?
            .port();
        // The secret goes in a configuration file only this user can read,
        // in the sandbox's own /tmp, rather than on aria2's command line,
        // which every process can read in /proc. aria2 reads it at start
        // and it is removed once aria2 answers.
        let conf = std::env::temp_dir().join(format!("danube-aria2-{}.conf", std::process::id()));
        std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&conf)
            .and_then(|mut f| writeln!(f, "rpc-secret={secret}"))
            .map_err(|e| format!("cannot write aria2's configuration: {e}"))?;
        let child = Command::new("aria2c")
            .arg(format!("--conf-path={}", conf.display()))
            .args(daemon_args(port, split, std::process::id()))
            .args(ca_certificate())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn();
        let child = match child {
            Ok(child) => child,
            Err(error) => {
                let _ = std::fs::remove_file(&conf);
                return Err(format!("cannot run aria2c: {error}"));
            }
        };
        let mut aria2 = Self {
            child,
            port,
            secret,
        };
        let started = Instant::now();
        let answer = loop {
            match aria2.call("aria2.getVersion", vec![]) {
                Ok(version) => break Ok(version),
                Err(error) if started.elapsed() > Duration::from_secs(5) => break Err(error),
                Err(_) => {}
            }
            if let Ok(Some(status)) = aria2.child.try_wait() {
                break Err(format!("aria2c exited ({status})"));
            }
            std::thread::sleep(Duration::from_millis(50));
        };
        let _ = std::fs::remove_file(&conf);
        let version = answer.map_err(|e| format!("aria2c did not answer: {e}"))?;
        info!(
            version = version["version"].as_str().unwrap_or("?"),
            port,
            split = split.0,
            per_server = split.1,
            "aria2 is up"
        );
        Ok(aria2)
    }

    fn token(&self) -> String {
        format!("token:{}", self.secret)
    }

    /// Calls `method` with the secret before `params`; aria2's answer, or
    /// its error's message.
    fn call(&self, method: &str, params: Vec<Value>) -> Result<Value, String> {
        let mut all = vec![json!(self.token())];
        all.extend(params);
        self.call_bare(method, Value::Array(all))
    }

    /// Calls `method` with `params` as they are (`system.multicall`
    /// carries the secret in each call it holds).
    fn call_bare(&self, method: &str, params: Value) -> Result<Value, String> {
        let body = json!({"jsonrpc": "2.0", "id": "danube", "method": method, "params": params}).to_string();
        let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port)).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(Duration::from_secs(5))).map_err(|e| e.to_string())?;
        write!(
            stream,
            "POST /jsonrpc HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .map_err(|e| e.to_string())?;
        let mut response = Vec::new();
        stream.read_to_end(&mut response).map_err(|e| e.to_string())?;
        answer(&response)
    }
}

impl Drop for Aria2 {
    fn drop(&mut self) {
        let _ = self.call("aria2.forceShutdown", vec![]);
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// aria2c's command line, past its configuration file.
fn daemon_args(port: u16, (split, per_server): (usize, usize), parent: u32) -> Vec<String> {
    vec![
        "--enable-rpc=true".into(),
        format!("--rpc-listen-port={port}"),
        // Loopback only: nothing off this computer reaches it.
        "--rpc-listen-all=false".into(),
        format!("--stop-with-process={parent}"),
        format!("--split={split}"),
        format!("--max-connection-per-server={per_server}"),
        // aria2 splits nothing under twice this, and its default (20 MiB)
        // would keep a file of tens of megabytes on one connection.
        "--min-split-size=1M".into(),
        // Danube names every file; aria2 must not rename one.
        "--auto-file-renaming=false".into(),
        // No space set aside first: on a file system without fallocate,
        // aria2's default writes the whole file with zeros before the
        // first byte arrives.
        "--file-allocation=none".into(),
        "--max-tries=5".into(),
        "--retry-wait=2".into(),
        // Nothing on a terminal and no log file: Danube reports progress,
        // and its own log says what went wrong.
        "--quiet=true".into(),
        "--console-log-level=error".into(),
        "--summary-interval=0".into(),
        "--show-console-readout=false".into(),
        // No session saved for aria2 to resume on its own later: a
        // download Danube did not finish is removed.
        "--force-save=false".into(),
    ]
}

/// The certificate authorities WebKit trusts (glib-networking reads the
/// system's bundle), for aria2 to trust the same ones.
fn ca_certificate() -> Option<String> {
    ["SSL_CERT_FILE", "NIX_SSL_CERT_FILE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain([PathBuf::from("/etc/ssl/certs/ca-certificates.crt")])
        .find(|p| p.is_file())
        .map(|p| format!("--ca-certificate={}", p.display()))
}

/// 128 random bits as hex, for aria2's RPC secret.
fn secret() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// The JSON-RPC answer in an HTTP response: its `result`, or its error's
/// message. aria2 answers errors with status 400 and the same JSON body,
/// so the status line is not read.
fn answer(response: &[u8]) -> Result<Value, String> {
    let text = String::from_utf8_lossy(response);
    let body = text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .ok_or("aria2 sent no HTTP body")?;
    let json: Value = serde_json::from_str(body).map_err(|e| format!("aria2 sent no JSON: {e}"))?;
    if let Some(error) = json.get("error") {
        return Err(error["message"].as_str().unwrap_or("aria2 refused").to_owned());
    }
    json.get("result").cloned().ok_or_else(|| "aria2 sent no result".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connections_follow_the_threads_and_cap_per_server() {
        assert_eq!(connections(Some(4)), (4, 4));
        assert_eq!(connections(Some(32)), (32, 16));
        assert_eq!(connections(Some(0)), (1, 1));
        let (split, per_server) = connections(None);
        assert!(split >= 1 && per_server <= MAX_PER_SERVER);
    }

    #[test]
    fn answers_and_errors() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 40\r\n\r\n{\"id\":\"danube\",\"jsonrpc\":\"2.0\",\"result\":\"2089b05ecca3d829\"}";
        assert_eq!(answer(ok), Ok(json!("2089b05ecca3d829")));
        let refused = b"HTTP/1.1 400 Bad Request\r\n\r\n{\"id\":1,\"jsonrpc\":\"2.0\",\"error\":{\"code\":1,\"message\":\"Unauthorized\"}}";
        assert_eq!(answer(refused), Err("Unauthorized".into()));
        assert!(answer(b"HTTP/1.1 200 OK\r\n").is_err());
    }

    #[test]
    fn statuses_become_updates() {
        let active = json!({"status": "active", "totalLength": "1000", "completedLength": "250", "downloadSpeed": "50", "connections": "4"});
        assert_eq!(
            Status::read(7, &active),
            Some(Update::Progress {
                id: 7,
                received: 250,
                total: Some(1000),
                speed: 50,
                connections: 4
            })
        );
        let unknown_size = json!({"status": "active", "totalLength": "0", "completedLength": "9"});
        assert!(matches!(Status::read(1, &unknown_size), Some(Update::Progress { total: None, .. })));
        assert_eq!(Status::read(1, &json!({"status": "complete"})), Some(Update::Done(1)));
        assert_eq!(Status::read(1, &json!({"status": "waiting"})), None);
        let failed = json!({"status": "error", "errorCode": "24", "errorMessage": "", "completedLength": "0"});
        assert_eq!(
            Status::read(2, &failed),
            Some(Update::Failed {
                id: 2,
                retry: true,
                started: true,
                message: "aria2 error 24".into()
            })
        );
    }

    #[test]
    fn options_carry_what_webkit_would_send() {
        let job = Job {
            id: 1,
            uri: "https://files.example/a.iso".into(),
            path: "/home/u/Downloads/a (1).iso".into(),
            cookie: Some("s=1; t=2".into()),
            referer: Some("https://files.example/".into()),
            user_agent: "Danube".into(),
        };
        let options = add_options(&job);
        assert_eq!(options["dir"], "/home/u/Downloads");
        assert_eq!(options["out"], "a (1).iso");
        assert_eq!(options["header"], json!(["Cookie: s=1; t=2"]));
        assert_eq!(options["referer"], "https://files.example/");
        let bare = add_options(&Job {
            cookie: None,
            referer: None,
            ..job
        });
        assert!(bare.get("header").is_none() && bare.get("referer").is_none());
    }

    #[test]
    fn the_daemon_listens_on_loopback_and_dies_with_danube() {
        let args = daemon_args(6800, (8, 8), 42);
        assert!(args.contains(&"--rpc-listen-all=false".to_owned()));
        assert!(args.contains(&"--stop-with-process=42".to_owned()));
        assert!(args.contains(&"--max-connection-per-server=8".to_owned()));
        assert!(!args.iter().any(|a| a.contains("secret")));
    }
}
