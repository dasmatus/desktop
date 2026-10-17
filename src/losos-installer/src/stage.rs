//! An online install's files, fetched with aria2 onto the disk being
//! installed before sysupdate reads them.
//!
//! aria2 splits each file over one connection per hardware thread, as many
//! as it allows to one server (16), so a server that caps each connection's
//! speed, as GitHub's release storage does, delivers the file that many
//! times faster than curl's one connection did. Each connection writes its
//! own range of the file wherever it is, so the file is only whole once all
//! of them are done: aria2 cannot hand it on to sysupdate as it arrives, the
//! way curl streamed it. The files have to be somewhere first, and the live
//! system's memory is not that place: a /usr is over 3 GiB, and the machine
//! may have 4 GiB in all.
//!
//! The disk being installed has the room. Until the installed system's first
//! boot creates root, `/home` and swap, everything after the /usr slots is
//! free, and those need 12 GiB of it, much more than a release takes. So the
//! files go into a scratch partition there ([`Scratch`]), which
//! [`crate::github::Mirror`] serves to sysupdate from 127.0.0.1. sysupdate
//! checks the signature and every hash exactly as before, and the scratch
//! partition is removed afterwards, leaving the free space first boot
//! expects.
//!
//! Only what the transfers will read is fetched: a release also carries the
//! installer ISO, the disk image and the other architecture's files.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

use crate::github::File;
use crate::install::{Event, command};

/// aria2 refuses more connections to one server than this
/// (`max-connection-per-server`).
pub const MAX_CONNECTIONS: usize = 16;

/// The partition label of the scratch partition, so a person who finds one
/// left by an install that was cut off can tell what it is.
pub const SCRATCH_LABEL: &str = "losos-download";

/// How many connections aria2 opens per file: one per hardware thread, as
/// `-x $(nproc)` would, within what aria2 accepts.
pub fn connections() -> usize {
    thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, MAX_CONNECTIONS)
}

/// The `MatchPattern=` of every transfer's `[Source]` in `templates`, the
/// names sysupdate will ask the mirror for.
pub fn source_patterns(templates: &Path) -> Result<Vec<String>, String> {
    let fail = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
    let mut paths: Vec<PathBuf> = fs::read_dir(templates)
        .map_err(|e| fail(templates, e))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "transfer"))
        .collect();
    paths.sort();
    let mut patterns = Vec::new();
    for path in paths {
        let text = fs::read_to_string(&path).map_err(|e| fail(&path, e))?;
        let mut source = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                source = line == "[Source]";
            } else if let Some(value) = line.strip_prefix("MatchPattern=").filter(|_| source) {
                // Several patterns are separated by spaces.
                patterns.extend(value.split_whitespace().map(str::to_owned));
            }
        }
    }
    if patterns.is_empty() {
        return Err(format!(
            "{}: no transfer names a source",
            templates.display()
        ));
    }
    Ok(patterns)
}

/// What `@v` stands for when `name` matches the sysupdate `pattern`, or
/// `None` when it does not match. `@v` is a version, `@@` a literal `@`,
/// and any other `@` field matches anything; a pattern without `@v`
/// matches with an empty version.
pub fn version_of(pattern: &str, name: &str) -> Option<String> {
    fn walk(pattern: &[u8], name: &[u8], version: &mut Option<String>) -> bool {
        match pattern {
            [] => name.is_empty(),
            [b'@', b'@', rest @ ..] => {
                name.first() == Some(&b'@') && walk(rest, &name[1..], version)
            }
            [b'@', field, rest @ ..] => {
                // sysupdate's version characters; '_' separates fields in
                // every name the OS writes, so it ends a version.
                let allowed =
                    |b: &u8| *field != b'v' || b.is_ascii_alphanumeric() || b".+~^-".contains(b);
                let longest = name.iter().take_while(|b| allowed(b)).count();
                (1..=longest).rev().any(|end| {
                    let mut inner = version.clone();
                    if *field == b'v' {
                        let text = String::from_utf8_lossy(&name[..end]).into_owned();
                        if version.as_ref().is_some_and(|v| *v != text) {
                            return false;
                        }
                        inner = Some(text);
                    }
                    walk(rest, &name[end..], &mut inner) && {
                        *version = inner;
                        true
                    }
                })
            }
            [c, rest @ ..] => name.first() == Some(c) && walk(rest, &name[1..], version),
        }
    }
    let mut version = None;
    walk(pattern.as_bytes(), name.as_bytes(), &mut version).then(|| version.unwrap_or_default())
}

/// The files of `files` sysupdate will read: the manifest and its signature,
/// and each transfer's file of `version`, or of the newest version every
/// transfer has a file of when `version` is `None`.
pub fn wanted(
    files: &BTreeMap<String, File>,
    patterns: &[String],
    version: Option<&str>,
) -> Result<BTreeMap<String, File>, String> {
    let per_pattern: Vec<Vec<(String, &String)>> = patterns
        .iter()
        .map(|pattern| {
            files
                .keys()
                .filter_map(|name| Some((version_of(pattern, name)?, name)))
                .collect()
        })
        .collect();
    let complete = |v: &str| {
        per_pattern
            .iter()
            .all(|found| found.iter().any(|(fv, _)| fv == v))
    };
    // CI's versions are `<YYYYMMDD>.<HHMMSS>`, all the same width, so the
    // newest is the greatest as text.
    let chosen = match version {
        Some(v) => complete(v).then(|| v.to_owned()),
        None => per_pattern
            .iter()
            .flatten()
            .map(|(v, _)| v)
            .filter(|v| complete(v))
            .max()
            .cloned(),
    }
    .ok_or_else(|| match version {
        Some(v) => format!("the release has no full set of files for {v}"),
        None => "the channel has no version with every file the transfers need".to_owned(),
    })?;

    let mut picked = BTreeMap::new();
    for name in ["SHA256SUMS", "SHA256SUMS.gpg"] {
        let file = files
            .get(name)
            .ok_or_else(|| format!("the release has no {name}"))?;
        picked.insert(name.to_owned(), file.clone());
    }
    for (v, name) in per_pattern.iter().flatten() {
        if *v == chosen {
            picked.insert((*name).clone(), files[*name].clone());
        }
    }
    Ok(picked)
}

/// The channel's index on the documentation site as files: one line each,
/// the name, its size and the URLs of its parts (website/release-index.js).
pub fn index_files(text: &str) -> BTreeMap<String, File> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.to_owned();
            let size = words.next()?.parse().ok()?;
            let urls: Vec<String> = words.map(str::to_owned).collect();
            (!urls.is_empty()).then_some((name, File { urls, size }))
        })
        .collect()
}

/// The name an asset at `url` is saved under: the last part of its path,
/// which is the asset's own name on GitHub.
pub fn local_name(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

/// How big a scratch partition must be to hold `files`: their bytes, ext4's
/// inode tables and reserved blocks, and some room on top, in whole MiB.
pub fn scratch_size(files: &BTreeMap<String, File>) -> u64 {
    const MIB: u64 = 1 << 20;
    let bytes: u64 = files.values().map(|f| f.size).sum();
    (bytes + bytes / 8 + 256 * MIB).div_ceil(MIB) * MIB
}

/// aria2's input file for `files`, every part saved under its own name.
pub fn aria2_input(files: &BTreeMap<String, File>) -> String {
    files
        .values()
        .flat_map(|file| &file.urls)
        .map(|url| format!("{url}\n  out={}\n", local_name(url)))
        .collect()
}

/// A file fetched into the scratch partition: its parts, in order, and how
/// many bytes they make together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Staged {
    pub paths: Vec<PathBuf>,
    pub size: u64,
}

/// Fetches `files` into `dir` with aria2, and returns where each one is.
/// `work` holds aria2's input file, which is not one of the files.
pub fn fetch(
    files: &BTreeMap<String, File>,
    dir: &Path,
    work: &Path,
    events: &Sender<Event>,
) -> Result<BTreeMap<String, Staged>, String> {
    let input = work.join("aria2-input");
    fs::write(&input, aria2_input(files)).map_err(|e| format!("{}: {e}", input.display()))?;
    let n = connections().to_string();
    let _ = events.send(Event::Line(format!(
        "Downloading {} files over {n} connections each",
        files.len()
    )));
    command(
        "aria2c",
        &[
            "--input-file",
            &input.to_string_lossy(),
            "--dir",
            &dir.to_string_lossy(),
            // One file at a time, each over every connection: the page
            // shows one file's progress, as it does for sysupdate's.
            "--max-concurrent-downloads=1",
            "--max-connection-per-server",
            &n,
            "--split",
            &n,
            // The UKI is 100 MiB or so; aria2's default of 20 MiB would
            // fetch it over five connections at most.
            "--min-split-size=1M",
            // A connection that breaks off is opened again and goes on from
            // where it stopped, its range's bytes already on the disk.
            "--continue=true",
            "--max-tries=10",
            "--retry-wait=3",
            "--file-allocation=falloc",
            "--auto-file-renaming=false",
            "--allow-overwrite=true",
            // Progress lines only: "[#1a2b3c 1.2GiB/2.4GiB(50%) CN:16 …]",
            // whose percentage the page shows (serve.rs). Notices print
            // GitHub's signed URLs, whose %3A would read as progress too.
            "--console-log-level=warn",
            "--summary-interval=0",
            "--download-result=hide",
            "--enable-color=false",
        ],
        false,
        events,
    )?;
    let mut staged = BTreeMap::new();
    for (name, file) in files {
        let paths: Vec<PathBuf> = file.urls.iter().map(|u| dir.join(local_name(u))).collect();
        let got: u64 = paths
            .iter()
            .map(|p| fs::metadata(p).map(|m| m.len()).unwrap_or(0))
            .sum();
        if got != file.size {
            return Err(format!(
                "{name}: aria2 fetched {got} bytes of {}",
                file.size
            ));
        }
        staged.insert(
            name.clone(),
            Staged {
                paths,
                size: file.size,
            },
        );
    }
    Ok(staged)
}

/// A partition in the disk's free space that holds what [`fetch`] fetched
/// until sysupdate has written it into the slots.
#[derive(Debug)]
pub struct Scratch {
    pub node: PathBuf,
    pub number: u32,
    pub mount: PathBuf,
}

/// The partition number at the end of a node's name: 5 for /dev/vda5 and
/// /dev/nvme0n1p5.
pub fn partition_number(node: &Path) -> Option<u32> {
    let name = node.file_name()?.to_str()?;
    let digits = name.len() - name.bytes().rev().take_while(u8::is_ascii_digit).count();
    name[digits..].parse().ok()
}

impl Scratch {
    /// Adds an ext4 partition of `size` bytes to `disk` and mounts it under
    /// `work`. repart makes it, from a definition of its own: it leaves the
    /// partitions it has no definition for as they are, tells the kernel
    /// about the new one and formats it, as it did the ESP.
    pub fn create(
        disk: &str,
        size: u64,
        work: &Path,
        events: &Sender<Event>,
    ) -> Result<Self, String> {
        let definitions = work.join("scratch.repart.d");
        let fail = |path: &Path, e: std::io::Error| format!("{}: {e}", path.display());
        fs::create_dir_all(&definitions).map_err(|e| fail(&definitions, e))?;
        let conf = definitions.join("90-download.conf");
        fs::write(
            &conf,
            format!(
                "[Partition]\nType=linux-generic\nLabel={SCRATCH_LABEL}\nFormat=ext4\n\
                 SizeMinBytes={size}\nSizeMaxBytes={size}\n"
            ),
        )
        .map_err(|e| fail(&conf, e))?;
        let table = command(
            "systemd-repart",
            &[
                "--dry-run=no",
                &format!("--definitions={}", definitions.display()),
                "--json=short",
                disk,
            ],
            true,
            events,
        )?;
        let node = scratch_node(&table)?;
        let number = partition_number(&node)
            .ok_or_else(|| format!("{}: no partition number", node.display()))?;
        command("udevadm", &["settle"], false, events)?;
        let mount = work.join("download");
        fs::create_dir_all(&mount).map_err(|e| fail(&mount, e))?;
        command(
            "mount",
            &[
                "-t",
                "ext4",
                &node.to_string_lossy(),
                &mount.to_string_lossy(),
            ],
            false,
            events,
        )?;
        Ok(Scratch {
            node,
            number,
            mount,
        })
    }

    /// Unmounts the partition and takes it off `disk` again, its ext4
    /// signature wiped first so nothing later mistakes the space for a
    /// filesystem.
    pub fn remove(self, disk: &str, events: &Sender<Event>) -> Result<(), String> {
        command("umount", &[&self.mount.to_string_lossy()], false, events)?;
        command(
            "wipefs",
            &["--all", &self.node.to_string_lossy()],
            false,
            events,
        )?;
        // The ESP is still mounted, so the kernel will not reread the whole
        // table; sfdisk only writes it, and partx drops the one partition.
        let number = self.number.to_string();
        command(
            "sfdisk",
            &["--no-reread", "--no-tell-kernel", "--delete", disk, &number],
            false,
            events,
        )?;
        command("partx", &["--delete", "--nr", &number, disk], false, events)?;
        command("udevadm", &["settle"], false, events)?;
        Ok(())
    }
}

/// The scratch partition's node, from repart's JSON summary of the table.
pub fn scratch_node(json: &str) -> Result<PathBuf, String> {
    let table: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("systemd-repart's output: {e}"))?;
    table
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["label"] == SCRATCH_LABEL)
        .and_then(|p| p["node"].as_str())
        .map(PathBuf::from)
        .ok_or_else(|| "systemd-repart reported no scratch partition".to_owned())
}
