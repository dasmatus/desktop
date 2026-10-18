//! Installing straight from the GitHub release the ISO was built for.
//!
//! CI publishes each version as a GitHub release (docs/releases.md): every
//! file of both architectures, each architecture's manifest as
//! `SHA256SUMS-<arch>` with its signature beside it, and a file of 2 GiB or
//! more as `<file>.part-00`, `.part-01` and on, since GitHub refuses larger
//! assets. systemd-sysupdate can read none of that as a channel: it wants a
//! `SHA256SUMS` and whole files under one URL.
//!
//! So the installer serves the release to it from 127.0.0.1 ([`Mirror`]).
//! `SHA256SUMS` and its signature are this architecture's, and a file asked
//! for by its own name is its parts one after the other, which aria2 has
//! fetched onto the disk being installed first ([`crate::stage`]). sysupdate
//! then checks the signature and every file's hash itself, exactly as it
//! does for the channel, and downloads into the slots as an update would.
//!
//! The release is found by its tag, which CI makes from the cadence the ISO
//! was built for and its version (`20261010.175322` is
//! `nightly-20261010T1753` for a nightly, `weekly-20261010T1753` for a
//! weekly). Releases of two cadences can share a version, but not their
//! files: the cadence a system follows is part of /usr.
use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::thread;

use serde_json::Value;

use crate::install::{Event, command};
use crate::stage::Staged;

/// Where GitHub's REST API answers.
const API: &str = "https://api.github.com";

/// One file of a release, as GitHub lists its assets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

/// A published release and what it carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

/// The tag CI gives the `channel` release of `version`, or `None` when the
/// version is not one CI writes (`<YYYYMMDD>.<HHMMSS>`).
pub fn release_tag(channel: &str, version: &str) -> Option<String> {
    let (date, time) = version.split_once('.')?;
    let digits = |s: &str, n| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    (digits(date, 8) && digits(time, 6)).then(|| format!("{channel}-{date}T{}", &time[..4]))
}

/// A release as the API describes it in JSON, or `None` for a draft or
/// anything without a tag.
pub fn parse_release(json: &Value) -> Option<Release> {
    if json["draft"].as_bool() == Some(true) {
        return None;
    }
    let assets = json["assets"]
        .as_array()?
        .iter()
        .filter_map(|asset| {
            Some(Asset {
                name: asset["name"].as_str()?.to_owned(),
                url: asset["browser_download_url"].as_str()?.to_owned(),
                size: asset["size"].as_u64()?,
            })
        })
        .collect();
    Some(Release {
        tag: json["tag_name"].as_str()?.to_owned(),
        prerelease: json["prerelease"].as_bool().unwrap_or(false),
        assets,
    })
}

/// Whether `release` is `version`'s, for `arch`: its manifest is there, and
/// so is a file named for the version and architecture.
pub fn carries(release: &Release, version: &str, arch: &str) -> bool {
    let manifest = format!("SHA256SUMS-{arch}");
    let named = format!("_{version}_{arch}");
    release.assets.iter().any(|a| a.name == manifest)
        && release.assets.iter().any(|a| a.name.contains(&named))
}

/// The API's answer at `path` as JSON.
fn api(path: &str, events: &Sender<Event>) -> Result<Value, String> {
    let body = command(
        "curl",
        &[
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--retry",
            "3",
            "--header",
            "Accept: application/vnd.github+json",
            &format!("{API}{path}"),
        ],
        true,
        events,
    )?;
    serde_json::from_str(&body).map_err(|e| format!("GitHub's answer for {path}: {e}"))
}

/// Finds the `channel` release of `version` for `arch` in `repo`
/// (`owner/name`).
///
/// pick looked through the newest hundred releases for a tag someone chose
/// by hand before the nightly. Every release is now tagged by CI from its
/// cadence and version, so its tag is known and asked for directly.
pub fn find(
    repo: &str,
    channel: &str,
    version: &str,
    arch: &str,
    events: &Sender<Event>,
) -> Result<Release, String> {
    let tag = release_tag(channel, version)
        .ok_or_else(|| format!("{version} is not a version CI tags releases with"))?;
    api(&format!("/repos/{repo}/releases/tags/{tag}"), events)
        .ok()
        .as_ref()
        .and_then(parse_release)
        .filter(|r| carries(r, version, arch))
        .ok_or_else(|| format!("{repo} has no {channel} release of {version} for {arch}"))
}

/// What the mirror answers for one name: the asset URLs to stream in order,
/// and how many bytes they make together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub urls: Vec<String>,
    pub size: u64,
}

/// The names sysupdate asks for, mapped onto `release`'s assets for `arch`.
/// `SHA256SUMS` and `SHA256SUMS.gpg` are the architecture's; a file split
/// into parts answers under its own name, its parts in order.
pub fn files(release: &Release, arch: &str) -> BTreeMap<String, File> {
    let manifest = format!("SHA256SUMS-{arch}");
    let signature = format!("{manifest}.gpg");
    let mut parts: BTreeMap<String, Vec<&Asset>> = BTreeMap::new();
    for asset in &release.assets {
        let name = if asset.name == manifest {
            "SHA256SUMS".to_owned()
        } else if asset.name == signature {
            "SHA256SUMS.gpg".to_owned()
        } else if asset.name.starts_with("SHA256SUMS") {
            // The other architecture's.
            continue;
        } else {
            match asset.name.rsplit_once(".part-") {
                Some((base, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
                    base.to_owned()
                }
                _ => asset.name.clone(),
            }
        };
        parts.entry(name).or_default().push(asset);
    }
    parts
        .into_iter()
        .map(|(name, mut assets)| {
            // `.part-00` before `.part-01`; same width, so text order is number
            // order.
            assets.sort_by(|a, b| a.name.cmp(&b.name));
            let file = File {
                urls: assets.iter().map(|a| a.url.clone()).collect(),
                size: assets.iter().map(|a| a.size).sum(),
            };
            (name, file)
        })
        .collect()
}

/// Fetched files served over HTTP on 127.0.0.1 for as long as
/// [`Mirror::serve`] runs.
pub struct Mirror {
    listener: TcpListener,
    files: BTreeMap<String, Staged>,
}

impl Mirror {
    pub fn new(files: BTreeMap<String, Staged>) -> io::Result<Self> {
        // Port 0: whichever is free. Only this machine can connect.
        let listener = TcpListener::bind("127.0.0.1:0")?;
        Ok(Mirror { listener, files })
    }

    /// The URL to give sysupdate as the transfers' source.
    pub fn url(&self) -> io::Result<String> {
        Ok(format!("http://{}/", self.listener.local_addr()?))
    }

    /// Answers requests while `work` runs, and returns what it returned.
    /// Each connection has a thread of its own in the same scope, so the
    /// signature and a file can be fetched at once, and none outlives `work`.
    pub fn serve<T>(&self, events: &Sender<Event>, work: impl FnOnce() -> T) -> T {
        let done = AtomicBool::new(false);
        thread::scope(|scope| {
            let done = &done;
            scope.spawn(move || {
                for stream in self.listener.incoming() {
                    if done.load(Ordering::Acquire) {
                        break;
                    }
                    let Ok(stream) = stream else { continue };
                    let events = events.clone();
                    scope.spawn(move || {
                        if let Err(e) = self.answer(stream) {
                            let _ = events.send(Event::Line(format!("release mirror: {e}")));
                        }
                    });
                }
            });
            let result = work();
            done.store(true, Ordering::Release);
            // accept() blocks until someone connects; this is that someone.
            if let Ok(addr) = self.listener.local_addr() {
                let _ = TcpStream::connect(addr);
            }
            result
        })
    }

    fn answer(&self, mut stream: TcpStream) -> io::Result<()> {
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut request = String::new();
        reader.read_line(&mut request)?;
        // The rest of the header says nothing this needs; it is read so the
        // client is not cut off while still sending it.
        let mut line = String::new();
        while reader.read_line(&mut line)? > 2 {
            line.clear();
        }
        let mut words = request.split_whitespace();
        let (method, path) = (words.next().unwrap_or(""), words.next().unwrap_or(""));
        let file = path
            .strip_prefix('/')
            .and_then(|name| self.files.get(name))
            .filter(|_| method == "GET" || method == "HEAD");
        let Some(file) = file else {
            stream.write_all(
                b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )?;
            return Ok(());
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n",
            file.size
        )?;
        if method == "HEAD" {
            return Ok(());
        }
        // One part after the other: sysupdate reads a split file whole.
        let copied = file
            .paths
            .iter()
            .try_for_each(|path| io::copy(&mut fs::File::open(path)?, &mut stream).map(|_| ()));
        let _ = stream.shutdown(Shutdown::Both);
        copied
    }
}
