//! LosOS cloud, the box's Nextcloud at `/nextcloud`: signing in for an app
//! password, and the WebDAV requests a run makes.
//!
//! Files go under `Backups/<computer>/` in the person's own files, at their
//! path in the home, with their modification time (`X-OC-MTime`). A file
//! deleted from the home is deleted there too, and lands in LosOS cloud's
//! trash, which with the old versions LosOS cloud keeps of every file that
//! changed is how an older copy comes back. A file larger than
//! [`CHUNKED_ABOVE`] goes up in chunks (Nextcloud's chunked upload v2),
//! since the box's web server takes a request of at most 512 MiB.

use std::{fs, io::Read, path::Path};

use base64::Engine;
use serde_json::Value;

use crate::{encode_path, encode_segment, Error};

/// Files larger than this are uploaded in chunks.
pub const CHUNKED_ABOVE: u64 = 128 << 20;

/// The size of each chunk; Nextcloud wants at least 5 MiB but for the last.
pub const CHUNK: u64 = 64 << 20;

/// The folder in the person's files that backups go into.
pub const FOLDER: &str = "Backups";

/// `Basic` authorization for `login` and `secret`.
pub fn basic(login: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{login}:{secret}"))
    )
}

/// LosOS cloud's address on `box_name`.
pub fn base(box_name: &str) -> String {
    format!("https://{box_name}/nextcloud")
}

/// Trades the person's password for an app password of its own, which
/// LosOS cloud lists under the account's security settings and which can
/// be revoked there without changing the password. Returns it with the
/// account's user ID.
pub fn sign_in(
    agent: &ureq::Agent,
    base: &str,
    login: &str,
    password: &str,
) -> Result<(String, String), Error> {
    let reply: Value = agent
        .get(&format!("{base}/ocs/v2.php/core/getapppassword"))
        .set("Authorization", &basic(login, password))
        .set("OCS-APIRequest", "true")
        .set("Accept", "application/json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(401, _) => {
                Error::new("LosOS cloud did not accept that name and password.")
            }
            e => Error::from(e),
        })?
        .into_json()?;
    let app_password = reply
        .pointer("/ocs/data/apppassword")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("LosOS cloud gave no app password."))?
        .to_owned();
    let user: Value = agent
        .get(&format!("{base}/ocs/v2.php/cloud/user"))
        .set("Authorization", &basic(login, &app_password))
        .set("OCS-APIRequest", "true")
        .set("Accept", "application/json")
        .call()?
        .into_json()?;
    let id = user
        .pointer("/ocs/data/id")
        .and_then(Value::as_str)
        .unwrap_or(login)
        .to_owned();
    Ok((app_password, id))
}

/// Revokes the app password being used.
pub fn sign_out(
    agent: &ureq::Agent,
    base: &str,
    login: &str,
    app_password: &str,
) -> Result<(), Error> {
    agent
        .delete(&format!("{base}/ocs/v2.php/core/apppassword"))
        .set("Authorization", &basic(login, app_password))
        .set("OCS-APIRequest", "true")
        .call()?;
    Ok(())
}

/// A person's files in LosOS cloud, under `Backups/<computer>`.
pub struct Cloud<'a> {
    agent: &'a ureq::Agent,
    base: String,
    user: String,
    auth: String,
    /// `Backups/<computer>`, encoded.
    root: String,
}

impl<'a> Cloud<'a> {
    /// The files of `user`, signed in as `login` with `app_password`, for
    /// the computer named `host`.
    pub fn new(
        agent: &'a ureq::Agent,
        base: String,
        login: &str,
        user: &str,
        app_password: &str,
        host: &str,
    ) -> Self {
        Self {
            agent,
            base,
            user: user.to_owned(),
            auth: basic(login, app_password),
            root: format!("{}/{}", encode_segment(FOLDER), encode_segment(host)),
        }
    }

    /// The WebDAV URL of `rel`, a path relative to the home.
    pub fn url(&self, rel: &str) -> String {
        let mut url = format!(
            "{}/remote.php/dav/files/{}/{}",
            self.base,
            encode_segment(&self.user),
            self.root
        );
        if !rel.is_empty() {
            url.push('/');
            url.push_str(&encode_path(rel));
        }
        url
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        self.agent
            .request(method, url)
            .set("Authorization", &self.auth)
    }

    /// Makes `Backups` and `Backups/<computer>` if they are not there.
    pub fn prepare(&self) -> Result<(), Error> {
        let folder = format!(
            "{}/remote.php/dav/files/{}/{}",
            self.base,
            encode_segment(&self.user),
            encode_segment(FOLDER)
        );
        self.mkcol_url(&folder)?;
        self.mkcol_url(&self.url(""))
    }

    fn mkcol_url(&self, url: &str) -> Result<(), Error> {
        match self.request("MKCOL", url).call() {
            // 405: it is already there.
            Ok(_) | Err(ureq::Error::Status(405, _)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Makes the folder `rel`, whose parent is there.
    pub fn mkcol(&self, rel: &str) -> Result<(), Error> {
        self.mkcol_url(&self.url(rel))
    }

    /// Deletes `rel`, a file or a folder with everything in it.
    pub fn delete(&self, rel: &str) -> Result<(), Error> {
        match self.request("DELETE", &self.url(rel)).call() {
            Ok(_) | Err(ureq::Error::Status(404, _)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }

    /// Uploads `path` as `rel`, `size` bytes changed at `mtime` (seconds),
    /// in one request or, when large, in chunks.
    pub fn put(&self, rel: &str, path: &Path, size: u64, mtime: i64) -> Result<(), Error> {
        if size > CHUNKED_ABOVE {
            return self.put_chunked(rel, path, size, mtime);
        }
        let file = fs::File::open(path)?;
        self.request("PUT", &self.url(rel))
            .set("Content-Length", &size.to_string())
            .set("X-OC-MTime", &mtime.to_string())
            .send(file.take(size))?;
        Ok(())
    }

    /// Nextcloud's chunked upload, version 2: a folder under `uploads`
    /// naming where the file goes, the chunks numbered from 1, then a MOVE
    /// of its `.file` there, which assembles them.
    fn put_chunked(&self, rel: &str, path: &Path, size: u64, mtime: i64) -> Result<(), Error> {
        let destination = self.url(rel);
        let id = format!("losos-backup-{:016x}-{}", crate::now(), fnv(rel));
        let uploads = format!(
            "{}/remote.php/dav/uploads/{}/{id}",
            self.base,
            encode_segment(&self.user)
        );
        self.request("MKCOL", &uploads)
            .set("Destination", &destination)
            .call()?;
        let result = (|| -> Result<(), Error> {
            let mut file = fs::File::open(path)?;
            let mut sent = 0;
            let mut number = 1;
            while sent < size {
                let len = CHUNK.min(size - sent);
                self.request("PUT", &format!("{uploads}/{number:05}"))
                    .set("Destination", &destination)
                    .set("OC-Total-Length", &size.to_string())
                    .set("Content-Length", &len.to_string())
                    .send((&mut file).take(len))?;
                sent += len;
                number += 1;
            }
            self.request("MOVE", &format!("{uploads}/.file"))
                .set("Destination", &destination)
                .set("OC-Total-Length", &size.to_string())
                .set("X-OC-MTime", &mtime.to_string())
                .set("Overwrite", "T")
                .call()?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.request("DELETE", &uploads).call();
        }
        result
    }
}

/// FNV-1a of `s`, which with the time makes an upload's folder name
/// unique enough.
fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_go_under_backups_and_the_computer() {
        let agent = ureq::agent();
        let cloud = Cloud::new(
            &agent,
            base("mattbox.local"),
            "matus@example.com",
            "matus",
            "x",
            "losos desktop",
        );
        assert_eq!(
            cloud.url("Documents/a b.txt"),
            "https://mattbox.local/nextcloud/remote.php/dav/files/matus/Backups/losos%20desktop/Documents/a%20b.txt"
        );
        assert_eq!(
            cloud.url(""),
            "https://mattbox.local/nextcloud/remote.php/dav/files/matus/Backups/losos%20desktop"
        );
        assert_eq!(basic("a", "b"), "Basic YTpi");
    }
}
