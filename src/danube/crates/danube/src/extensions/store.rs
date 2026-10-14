//! Where installed extensions live and how one gets there. Each is unpacked
//! under `~/.local/share/danube/extensions/<id>/`, the id being the one
//! Chrome would give it ([`Manifest::id`]), so the same extension installed
//! twice replaces itself. `~/.config/danube/extensions` lists one
//! `<id> enabled|disabled` per line; an extension absent from it is enabled,
//! so a fresh install needs no line.
//!
//! An install takes a `.zip`, a Chrome `.crx` (a zip after its header), a
//! Firefox `.xpi` (a zip under another name) or an unpacked directory. The
//! zip reader is this file's own: the browser needs the central directory
//! and deflate, which `flate2` has, and nothing else from a zip crate.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use flate2::read::DeflateDecoder;
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::manifest::{self, Manifest};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{path}: {source}")]
    Manifest {
        path: PathBuf,
        source: manifest::Error,
    },
    #[error("{0} is not a zip, crx, xpi or directory")]
    Format(PathBuf),
    #[error("the archive is malformed: {0}")]
    Zip(&'static str),
    #[error("the archive holds a path it may not: {0}")]
    Path(String),
    #[error("{0}: no such extension")]
    Unknown(String),
}

/// One installed extension: where it is and whether the person turned it
/// off.
#[derive(Debug)]
pub struct Installed {
    pub id: String,
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub enabled: bool,
}

/// The two paths above, kept together so the runtime and the `install-extension`
/// command agree on them.
#[derive(Clone, Debug)]
pub struct Store {
    pub root: PathBuf,
    pub state: PathBuf,
}

impl Store {
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join(".local/share/danube/extensions"),
            state: home.join(".config/danube/extensions"),
        }
    }

    /// Every installed extension whose manifest still parses, in id order.
    /// `lang` picks the `_locales` messages its strings are shown in.
    pub fn list(&self, lang: &str) -> Vec<Installed> {
        let state = self.read_state();
        let Ok(entries) = fs::read_dir(&self.root) else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for entry in entries.flatten() {
            let dir = entry.path();
            if !dir.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            match Manifest::load(&dir, lang) {
                Ok(manifest) => found.push(Installed {
                    enabled: state.get(&id).copied().unwrap_or(true),
                    id,
                    dir,
                    manifest,
                }),
                // A half-written or foreign directory stays out of the way
                // rather than breaking every other extension.
                Err(error) => tracing::warn!("{}: {error}", dir.display()),
            }
        }
        found.sort_by(|a, b| a.id.cmp(&b.id));
        found
    }

    /// Installs `source`, replacing an earlier copy of the same extension,
    /// and returns what landed.
    pub fn install(&self, source: &Path, lang: &str) -> Result<Installed, Error> {
        fs::create_dir_all(&self.root)?;
        // Unpack beside the final place so the rename at the end stays on
        // one filesystem, and so a failed install leaves nothing under an id.
        let staging = self.root.join(format!(".installing-{}", std::process::id()));
        let _ = fs::remove_dir_all(&staging);
        fs::create_dir_all(&staging)?;
        let result = self.stage(source, &staging, lang);
        let (id, manifest) = match result {
            Ok(found) => found,
            Err(error) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(error);
            }
        };
        let dir = self.root.join(&id);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        fs::rename(&staging, &dir)?;
        let enabled = self.read_state().get(&id).copied().unwrap_or(true);
        Ok(Installed {
            id,
            dir,
            manifest,
            enabled,
        })
    }

    fn stage(&self, source: &Path, staging: &Path, lang: &str) -> Result<(String, Manifest), Error> {
        let mut key = None;
        if source.is_dir() {
            copy_tree(source, staging)?;
        } else {
            let bytes = fs::read(source)?;
            let Some((zip, crx_key)) = zip_payload(&bytes) else {
                return Err(Error::Format(source.to_path_buf()));
            };
            extract(zip, staging)?;
            key = crx_key.map(<[u8]>::to_vec);
        }
        // A zip made of a folder (`ext/manifest.json`) unpacks one level
        // down; hoist that level when it is the only thing there.
        hoist_single_directory(staging)?;
        if let Some(key) = key {
            keep_key(staging, &key)?;
        }
        let manifest = Manifest::load(staging, lang).map_err(|source| Error::Manifest {
            path: staging.join("manifest.json"),
            source,
        })?;
        Ok((manifest.id(), manifest))
    }

    /// Removes an extension's files and its state line.
    pub fn uninstall(&self, id: &str) -> Result<(), Error> {
        let dir = self.root.join(id);
        if !dir.is_dir() {
            return Err(Error::Unknown(id.to_string()));
        }
        fs::remove_dir_all(&dir)?;
        let mut state = self.read_state();
        state.remove(id);
        self.write_state(&state)
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), Error> {
        let mut state = self.read_state();
        if enabled {
            // Enabled is the default, so the line goes rather than stays.
            state.remove(id);
        } else {
            state.insert(id.to_string(), false);
        }
        self.write_state(&state)
    }

    fn read_state(&self) -> BTreeMap<String, bool> {
        let mut state = BTreeMap::new();
        if let Ok(text) = fs::read_to_string(&self.state) {
            for line in text.lines() {
                let mut words = line.split_whitespace();
                if let (Some(id), Some(word)) = (words.next(), words.next()) {
                    state.insert(id.to_string(), word != "disabled");
                }
            }
        }
        state
    }

    fn write_state(&self, state: &BTreeMap<String, bool>) -> Result<(), Error> {
        if let Some(parent) = self.state.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = String::new();
        for (id, enabled) in state {
            text.push_str(id);
            text.push(' ');
            text.push_str(if *enabled { "enabled" } else { "disabled" });
            text.push('\n');
        }
        // Written whole then renamed, so a crash leaves the old list, never
        // half of the new one.
        let temp = self.state.with_extension("tmp");
        fs::File::create(&temp)?.write_all(text.as_bytes())?;
        fs::rename(&temp, &self.state)?;
        Ok(())
    }
}

/// The zip inside `bytes`: the file itself, or what follows a crx header.
/// crx3 (`Cr24`, version 3) carries a protobuf header whose length is the
/// next little-endian u32; crx2 carries a public key and a signature with
/// their lengths in the header. Neither signature is checked: an install
/// is a command the person ran on a file they chose, like unzipping it.
fn zip_payload(bytes: &[u8]) -> Option<(&[u8], Option<&[u8]>)> {
    if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
        return Some((bytes, None));
    }
    if !bytes.starts_with(b"Cr24") || bytes.len() < 16 {
        return None;
    }
    let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().ok()?).try_into().ok();
    let version: usize = u32_at(4)?;
    let (start, key) = match version {
        // CRX2: the public key, then the signature, then the zip.
        2 => {
            let key_len: usize = u32_at(8)?;
            (16 + key_len + u32_at(12)?, bytes.get(16..16 + key_len))
        }
        // CRX3: a protobuf header, then the zip.
        3 => {
            let header_len: usize = u32_at(8)?;
            (12 + header_len, bytes.get(12..12 + header_len).and_then(crx3_key))
        }
        _ => return None,
    };
    bytes.get(start..).filter(|rest| rest.starts_with(b"PK")).map(|zip| (zip, key))
}

/// The public key a CRX3 header names the extension by. The header
/// (`CrxFileHeader`) holds every key that signed the file under field 2 and
/// the id under 10000 (`SignedData.crx_id`); the id's key is the one whose
/// SHA-256 starts with it, the store's own signature being another.
fn crx3_key(header: &[u8]) -> Option<&[u8]> {
    let fields = protobuf_fields(header);
    let crx_id = fields
        .iter()
        .find(|(number, _)| *number == 10000)
        .and_then(|(_, signed)| protobuf_fields(signed).into_iter().find(|(number, _)| *number == 1))
        .map(|(_, id)| id)?;
    fields
        .iter()
        .filter(|(number, _)| *number == 2)
        .filter_map(|(_, proof)| protobuf_fields(proof).into_iter().find(|(number, _)| *number == 1))
        .map(|(_, key)| key)
        .find(|key| Sha256::digest(key).starts_with(crx_id))
}

/// The length-delimited fields of a protobuf message, as (number, bytes);
/// varints are skipped and anything malformed ends the list. A CRX header
/// needs no more of protobuf than this.
fn protobuf_fields(mut bytes: &[u8]) -> Vec<(u64, &[u8])> {
    fn varint(bytes: &mut &[u8]) -> Option<u64> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let (&byte, rest) = bytes.split_first()?;
            *bytes = rest;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Some(value);
            }
        }
        None
    }
    let mut fields = Vec::new();
    while let Some(tag) = varint(&mut bytes) {
        match tag & 7 {
            0 => {
                if varint(&mut bytes).is_none() {
                    break;
                }
            }
            2 => {
                let Some(len) = varint(&mut bytes).and_then(|l| usize::try_from(l).ok()) else { break };
                let Some(value) = bytes.get(..len) else { break };
                fields.push((tag >> 3, value));
                bytes = &bytes[len..];
            }
            _ => break,
        }
    }
    fields
}

/// Writes a CRX's public key into its unpacked manifest, as Chrome does,
/// when the manifest has none: a store's manifest leaves it out, and the
/// id, which the extension's own pages and its servers rely on, comes from
/// it.
fn keep_key(dir: &Path, key: &[u8]) -> Result<(), Error> {
    use base64::Engine as _;
    let path = dir.join("manifest.json");
    let Ok(text) = fs::read_to_string(&path) else { return Ok(()) };
    let Ok(Value::Object(mut raw)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
        return Ok(());
    };
    if raw.contains_key("key") {
        return Ok(());
    }
    raw.insert("key".into(), base64::engine::general_purpose::STANDARD.encode(key).into());
    fs::write(&path, serde_json::to_vec_pretty(&raw).unwrap_or_default())?;
    Ok(())
}

/// Unpacks `zip` into `into`, refusing entries that would land outside it.
fn extract(zip: &[u8], into: &Path) -> Result<(), Error> {
    for entry in central_directory(zip)? {
        let relative = safe_relative(&entry.name)?;
        let target = into.join(&relative);
        if entry.name.ends_with('/') {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = entry.data(zip)?;
        fs::write(&target, data)?;
    }
    Ok(())
}

/// `name` as a path with no `..`, no root and no drive, or the error that
/// says why it was refused: an archive from the web does not get to write
/// outside its directory.
pub(super) fn safe_relative(name: &str) -> Result<PathBuf, Error> {
    let path = Path::new(name);
    let mut relative = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            _ => return Err(Error::Path(name.to_string())),
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(Error::Path(name.to_string()));
    }
    Ok(relative)
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    uncompressed: usize,
    header_offset: usize,
}

impl Entry {
    /// The entry's bytes, inflated when stored deflated. The local header's
    /// name and extra field lengths are read there, since they may differ
    /// from the central directory's.
    fn data(&self, zip: &[u8]) -> Result<Vec<u8>, Error> {
        let at = self.header_offset;
        let header = zip.get(at..at + 30).ok_or(Error::Zip("local header past the end"))?;
        if &header[0..4] != b"PK\x03\x04" {
            return Err(Error::Zip("local header signature"));
        }
        let name_len = u16::from_le_bytes([header[26], header[27]]) as usize;
        let extra_len = u16::from_le_bytes([header[28], header[29]]) as usize;
        let start = at + 30 + name_len + extra_len;
        let raw = zip
            .get(start..start + self.compressed)
            .ok_or(Error::Zip("entry data past the end"))?;
        match self.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(self.uncompressed);
                DeflateDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|_| Error::Zip("deflate stream"))?;
                Ok(out)
            }
            _ => Err(Error::Zip("compression method other than store or deflate")),
        }
    }
}

/// The entries the end-of-central-directory record points at. ZIP64 is not
/// read: an extension that needs it is larger than any browser would load.
fn central_directory(zip: &[u8]) -> Result<Vec<Entry>, Error> {
    let eocd = zip
        .len()
        .checked_sub(22)
        .and_then(|last| (0..=last.min(65535 + 1)).map(|back| last - back).find(|&at| zip[at..].starts_with(b"PK\x05\x06")))
        .ok_or(Error::Zip("no end of central directory"))?;
    let u16_at = |at: usize| u16::from_le_bytes([zip[at], zip[at + 1]]) as usize;
    let u32_at = |at: usize| u32::from_le_bytes(zip[at..at + 4].try_into().unwrap()) as usize;
    let count = u16_at(eocd + 10);
    let mut at = u32_at(eocd + 16);
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let header = zip.get(at..at + 46).ok_or(Error::Zip("central directory past the end"))?;
        if &header[0..4] != b"PK\x01\x02" {
            return Err(Error::Zip("central directory signature"));
        }
        let method = u16::from_le_bytes([header[10], header[11]]);
        let compressed = u32_at(at + 20);
        let uncompressed = u32_at(at + 24);
        let name_len = u16_at(at + 28);
        let extra_len = u16_at(at + 30);
        let comment_len = u16_at(at + 32);
        let header_offset = u32_at(at + 42);
        let name = zip
            .get(at + 46..at + 46 + name_len)
            .ok_or(Error::Zip("entry name past the end"))?;
        entries.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method,
            compressed,
            uncompressed,
            header_offset,
        });
        at += 46 + name_len + extra_len + comment_len;
    }
    Ok(entries)
}

/// When `dir` holds one directory and nothing else, its contents move up
/// and it goes.
fn hoist_single_directory(dir: &Path) -> Result<(), Error> {
    if dir.join("manifest.json").exists() {
        return Ok(());
    }
    let entries: Vec<_> = fs::read_dir(dir)?.collect::<Result<_, _>>()?;
    let [only] = entries.as_slice() else {
        return Ok(());
    };
    let inner = only.path();
    if !inner.is_dir() || !inner.join("manifest.json").exists() {
        return Ok(());
    }
    // Moved aside first: a child named like its parent would otherwise
    // collide on the way up.
    let aside = dir.join(".hoisting");
    fs::rename(&inner, &aside)?;
    for entry in fs::read_dir(&aside)? {
        let entry = entry?;
        fs::rename(entry.path(), dir.join(entry.file_name()))?;
    }
    fs::remove_dir(&aside)?;
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), Error> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type()?;
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target)?;
        }
        // Symlinks are left behind: one pointing outside the extension
        // would otherwise serve that file at chrome-extension://.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of `files`, stored or deflated, written as Python's zipfile
    /// would: local headers, central directory, end record.
    fn zip_of(files: &[(&str, &[u8])], deflate: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let body = if deflate {
                let mut encoder = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                encoder.write_all(data).unwrap();
                encoder.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            let mut local = Vec::new();
            local.extend_from_slice(b"PK\x03\x04");
            local.extend_from_slice(&[20, 0, 0, 0]);
            local.extend_from_slice(&method.to_le_bytes());
            local.extend_from_slice(&[0; 8]); // time, date, crc
            local.extend_from_slice(&(body.len() as u32).to_le_bytes());
            local.extend_from_slice(&(data.len() as u32).to_le_bytes());
            local.extend_from_slice(&(name.len() as u16).to_le_bytes());
            local.extend_from_slice(&0u16.to_le_bytes());
            local.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&local);
            out.extend_from_slice(&body);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]); // extra, comment, disk, attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let start = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&start.to_le_bytes());
        out.extend_from_slice(&[0; 2]);
        out
    }

    const MANIFEST: &[u8] = br#"{"manifest_version": 3, "name": "Test", "version": "1.0"}"#;

    fn temp_home(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("danube-store-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn installs_a_zip_under_its_chrome_id() {
        let home = temp_home("zip");
        let zip = zip_of(&[("manifest.json", MANIFEST), ("bg.js", b"1")], true);
        let file = home.join("ext.zip");
        fs::write(&file, zip).unwrap();
        let store = Store::new(&home);
        let installed = store.install(&file, "en").unwrap();
        assert_eq!(installed.id.len(), 32);
        assert!(installed.dir.join("bg.js").exists());
        assert!(installed.enabled);
        assert_eq!(store.list("en").len(), 1);
        store.set_enabled(&installed.id, false).unwrap();
        assert!(!store.list("en")[0].enabled);
        store.uninstall(&installed.id).unwrap();
        assert!(store.list("en").is_empty());
    }

    #[test]
    fn hoists_a_folder_and_reads_crx() {
        let home = temp_home("crx");
        let zip = zip_of(&[("ext/", b""), ("ext/manifest.json", MANIFEST)], false);
        let mut crx = b"Cr24".to_vec();
        crx.extend_from_slice(&3u32.to_le_bytes());
        crx.extend_from_slice(&4u32.to_le_bytes());
        crx.extend_from_slice(b"hdr!");
        crx.extend_from_slice(&zip);
        let file = home.join("ext.crx");
        fs::write(&file, crx).unwrap();
        let installed = Store::new(&home).install(&file, "en").unwrap();
        assert!(installed.dir.join("manifest.json").exists());
        assert!(!installed.dir.join("ext").exists());
    }

    /// A protobuf length-delimited field.
    fn field(number: u64, value: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        for mut n in [number << 3 | 2, value.len() as u64] {
            while n >= 0x80 {
                out.push(n as u8 | 0x80);
                n >>= 7;
            }
            out.push(n as u8);
        }
        out.extend_from_slice(value);
        out
    }

    #[test]
    fn names_a_crx3_by_its_header_key() {
        let home = temp_home("crxkey");
        let (store_key, own_key) = (b"the store's key".as_slice(), b"the extension's key".as_slice());
        let crx_id = &Sha256::digest(own_key)[..16];
        let mut header = field(2, &field(1, store_key));
        header.extend(field(2, &field(1, own_key)));
        header.extend(field(10000, &field(1, crx_id)));
        let mut crx = b"Cr24".to_vec();
        crx.extend_from_slice(&3u32.to_le_bytes());
        crx.extend_from_slice(&(header.len() as u32).to_le_bytes());
        crx.extend_from_slice(&header);
        crx.extend(zip_of(&[("manifest.json", MANIFEST)], false));
        let file = home.join("ext.crx");
        fs::write(&file, crx).unwrap();
        let installed = Store::new(&home).install(&file, "en").unwrap();
        let expected: String = crx_id.iter().flat_map(|b| [b >> 4, b & 15]).map(|n| (b'a' + n) as char).collect();
        assert_eq!(installed.id, expected);
        // Kept in the manifest, so the id survives the next start.
        assert_eq!(installed.manifest.id(), expected);
    }

    #[test]
    fn refuses_escaping_paths() {
        let home = temp_home("escape");
        let zip = zip_of(&[("../evil.js", b"1"), ("manifest.json", MANIFEST)], false);
        let file = home.join("ext.zip");
        fs::write(&file, zip).unwrap();
        let store = Store::new(&home);
        assert!(matches!(store.install(&file, "en"), Err(Error::Path(_))));
        assert!(!home.join("evil.js").exists());
        assert!(store.list("en").is_empty());
    }
}
