//! The way to the box, and the box's certificate.
//!
//! On a PC the one machine that talks to LosOS boxes is the LosOS boxes
//! machine, and every other machine reaches a box through it: its
//! `losos-boxes route` takes a connection on port 80 or 443, reads the box
//! it is for from the `Host` header or the TLS server name, and passes it
//! on to that box as it is. The host's `losos-boxes relay` writes the boxes
//! each such machine sees into `/run/losos-boxes/hosts/<machine>`, one
//! `<machine's address> <box>.local` line each. The backups use those
//! files the same way libvirt's DNS does for the machines: the box's name
//! goes to that address, so the box's own certificate is what answers.
//!
//! That directory exists on every PC with machines, and is empty while no
//! LosOS boxes machine runs; then a run starts one the person has (and
//! stops it again after), rather than going round it to the box. A system
//! without the directory, a phone, has no machines and reaches the box by
//! its name, through resolved's mDNS.
//!
//! The box's certificate is self-signed for its `.local` name. It is
//! fetched over plain HTTP when the box is chosen, its fingerprint shown to
//! compare with the box's wizard, and from then on every connection must
//! present exactly that certificate.

use std::{
    io,
    net::{IpAddr, SocketAddr, ToSocketAddrs},
    path::Path,
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use base64::Engine;
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    crypto::CryptoProvider,
    pki_types::{CertificateDer, ServerName, UnixTime},
    DigitallySignedStruct, SignatureScheme,
};
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use crate::Error;

/// Where the host's relay names the boxes each LosOS boxes machine sees.
pub const HOSTS_DIR: &str = "/run/losos-boxes/hosts";

/// The template a LosOS boxes machine is made from, as pm records it in
/// the machine's libvirt definition.
pub const MACHINE_TEMPLATE: &str = "losos-boxes";

/// How long a LosOS boxes machine started for a run has to find the box.
pub const MACHINE_WAIT: Duration = Duration::from_secs(300);

/// A box, and the address its connections go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route {
    /// The box's name, which TLS and `Host` carry.
    pub name: String,
    /// The LosOS boxes machine's address, or the box's own.
    pub address: IpAddr,
    /// The LosOS boxes machine it goes through, if any.
    pub machine: Option<String>,
}

/// Why there is no way to the box now.
#[derive(Debug, PartialEq, Eq)]
pub enum Missing {
    /// This system routes boxes through a LosOS boxes machine, and none
    /// that runs lists the box.
    NotRouted,
    /// The name did not resolve: the box is not on this network.
    NotFound(String),
}

/// The names in a hosts file the relay wrote, each with its address.
pub fn parse_hosts(text: &str) -> impl Iterator<Item = (IpAddr, &str)> {
    text.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        let address = words.next()?.parse().ok()?;
        Some((address, words.next()?))
    })
}

/// A box name as the relay writes it: lowercase, without a trailing dot.
pub fn normalize(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// The boxes the running LosOS boxes machines see, as `(box, machine,
/// address)`, or `None` on a system that routes no boxes through one.
pub fn routed(hosts_dir: &Path) -> Option<Vec<(String, String, IpAddr)>> {
    let entries = std::fs::read_dir(hosts_dir).ok()?;
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let machine = entry.file_name().to_string_lossy().into_owned();
        // The relay writes each file beside the directory and renames it
        // in, but a dot file is never one of its lists.
        if machine.starts_with('.') {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for (address, name) in parse_hosts(&text) {
            found.push((normalize(name), machine.clone(), address));
        }
    }
    found.sort();
    Some(found)
}

/// The way to `name` now.
pub fn find(name: &str, hosts_dir: &Path) -> Result<Route, Missing> {
    let name = normalize(name);
    if let Some(boxes) = routed(hosts_dir) {
        return boxes
            .into_iter()
            .find(|(n, _, _)| *n == name)
            .map(|(name, machine, address)| Route {
                name,
                address,
                machine: Some(machine),
            })
            .ok_or(Missing::NotRouted);
    }
    match (name.as_str(), 443).to_socket_addrs() {
        Ok(mut addrs) => addrs
            .next()
            .map(|a| Route {
                name: name.clone(),
                address: a.ip(),
                machine: None,
            })
            .ok_or_else(|| Missing::NotFound(format!("{name} has no address"))),
        Err(e) => Err(Missing::NotFound(format!("{name}: {e}"))),
    }
}

/// The boxes to offer on the Backups page: the ones the LosOS boxes
/// machines see, or on a system without them, what `losos-boxes list`
/// finds over mDNS in a few seconds.
pub fn offered(hosts_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = match routed(hosts_dir) {
        Some(boxes) => boxes.into_iter().map(|(name, _, _)| name).collect(),
        None => Command::new("losos-boxes")
            .arg("list")
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| {
                // `name<TAB>url<TAB>version`, the url being http://<host>/.
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .filter_map(|l| l.split('\t').nth(1))
                    .filter_map(|url| url.strip_prefix("http://"))
                    .map(|host| normalize(host.trim_end_matches('/')))
                    .collect()
            })
            .unwrap_or_default(),
    };
    names.sort();
    names.dedup();
    names
}

/// The template pm recorded in a machine's libvirt definition
/// (`<pm:template name='losos-boxes' .../>`).
pub fn machine_template(xml: &str) -> Option<&str> {
    let at = xml.find(":template ").or_else(|| xml.find("<template "))?;
    let rest = &xml[at..xml[at..].find('>').map_or(xml.len(), |end| at + end)];
    let value = &rest[rest.find("name=")? + "name=".len()..];
    let quote = value.chars().next().filter(|q| *q == '\'' || *q == '"')?;
    let value = &value[1..];
    Some(&value[..value.find(quote)?])
}

fn virsh(args: &[&str]) -> Option<String> {
    let output = Command::new("virsh")
        .args(["-r", "-q", "-c", "qemu:///system"])
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The LosOS boxes machines this host has, running or not, by name.
pub fn machines() -> Vec<String> {
    let Some(names) = virsh(&["list", "--all", "--name"]) else {
        return Vec::new();
    };
    names
        .lines()
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .filter(|n| {
            virsh(&["dumpxml", "--inactive", n])
                .is_some_and(|xml| machine_template(&xml) == Some(MACHINE_TEMPLATE))
        })
        .map(str::to_owned)
        .collect()
}

/// Whether libvirt has `name` running.
pub fn machine_running(name: &str) -> bool {
    virsh(&["domstate", name]).is_some_and(|s| s.trim() == "running")
}

/// Runs `pm machine <verb> <name>`.
pub fn pm_machine(verb: &str, name: &str) -> Result<(), Error> {
    let output = Command::new("pm")
        .args(["machine", verb, name])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| Error::new(format!("cannot run pm: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
        Err(Error::new(format!(
            "pm machine {verb} {name} failed: {}",
            last.unwrap_or("no reason given").trim()
        )))
    }
}

/// Starts the first stopped LosOS boxes machine and waits until it lists
/// `name`. Returns the route and the machine started, which the run stops
/// again; or why it could not.
pub fn start_machine_for(name: &str, hosts_dir: &Path) -> Result<(Route, String), Error> {
    let all = machines();
    let Some(machine) = all.iter().find(|m| !machine_running(m)) else {
        return Err(Error::new(if all.is_empty() {
            "This computer reaches LosOS boxes through a LosOS boxes machine, and has none. Make one in Machines from the losos-boxes template."
                .to_owned()
        } else {
            format!("The LosOS boxes machine is running and does not see {name}.")
        }));
    };
    info!(%machine, "starting the LosOS boxes machine to reach the box");
    pm_machine("start", machine)?;
    let deadline = Instant::now() + MACHINE_WAIT;
    while Instant::now() < deadline {
        if let Ok(route) = find(name, hosts_dir) {
            return Ok((route, machine.clone()));
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    let _ = pm_machine("stop", machine);
    Err(Error::new(format!(
        "The LosOS boxes machine {machine} did not see {name} within {} minutes.",
        MACHINE_WAIT.as_secs() / 60
    )))
}

/// An agent whose every connection goes to `route`'s address, whatever
/// name the URL has, so the name is still what `Host` and TLS carry.
fn builder(route: &Route) -> ureq::AgentBuilder {
    let address = route.address;
    ureq::AgentBuilder::new()
        .resolver(move |netloc: &str| -> io::Result<Vec<SocketAddr>> {
            let port = netloc
                .rsplit_once(':')
                .and_then(|(_, p)| p.parse().ok())
                .unwrap_or(443);
            Ok(vec![SocketAddr::new(address, port)])
        })
        .timeout_connect(Duration::from_secs(15))
        // A chunk of a large file, or Nextcloud assembling one, can take a
        // while on a small box.
        .timeout_read(Duration::from_secs(300))
        .timeout_write(Duration::from_secs(300))
        .redirects(0)
        .user_agent(concat!("losos-backup/", env!("CARGO_PKG_VERSION")))
}

/// An agent for plain HTTP, which only fetches the box's certificate.
pub fn plain(route: &Route) -> ureq::Agent {
    builder(route).build()
}

/// An agent that talks to the box over TLS, accepting only `cert`.
pub fn pinned(route: &Route, cert: &[u8]) -> Result<ureq::Agent, Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Error::new(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned {
            cert: cert.to_vec(),
            provider,
        }))
        .with_no_client_auth();
    Ok(builder(route).tls_config(Arc::new(config)).build())
}

/// Accepts the one certificate it holds, and checks the handshake's
/// signatures against it as any verifier would.
#[derive(Debug)]
struct Pinned {
    cert: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.cert.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            debug!(
                presented = fingerprint(end_entity.as_ref()),
                "not the box's certificate"
            );
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// The DER of the first certificate in `pem`.
pub fn pem_to_der(pem: &str) -> Option<Vec<u8>> {
    let body = pem
        .split("-----BEGIN CERTIFICATE-----")
        .nth(1)?
        .split("-----END CERTIFICATE-----")
        .next()?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD.decode(b64).ok()
}

/// A certificate's SHA-256 fingerprint as `openssl x509 -fingerprint
/// -sha256` and so the box's wizard spell it: `AB:CD:…`.
pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Fetches the box's certificate over plain HTTP, through `route`, and
/// checks it against the fingerprint in the box's `/setup/state.json`.
/// Both come unauthenticated; matching each other only rules out a
/// truncated or mixed-up answer, and the fingerprint is shown so the
/// person can compare it with the box's own pages.
pub fn fetch_certificate(route: &Route) -> Result<String, Error> {
    let agent = plain(route);
    let base = format!("http://{}", route.name);
    let pem = agent
        .get(&format!("{base}/setup/losos-ca.crt"))
        .call()?
        .into_string()?;
    let der = pem_to_der(&pem).ok_or_else(|| {
        Error::new(format!(
            "{} answered /setup/losos-ca.crt with no certificate: is it a LosOS box?",
            route.name
        ))
    })?;
    let state: serde_json::Value = agent
        .get(&format!("{base}/setup/state.json"))
        .call()?
        .into_json()?;
    let stated = state
        .get("fingerprint")
        .and_then(|f| f.as_str())
        .and_then(|f| f.strip_prefix("sha256:"))
        .map(|hex| hex.replace(':', "").to_ascii_uppercase());
    let ours = fingerprint(&der).replace(':', "");
    if stated.as_deref() != Some(ours.as_str()) {
        return Err(Error::new(format!(
            "The certificate {} sent does not match the fingerprint it states.",
            route.name
        )));
    }
    Ok(pem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_relays_hosts_files() {
        let dir = std::env::temp_dir().join(format!("losos-backup-hosts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(find("mattbox.local", &dir), Err(Missing::NotRouted));
        std::fs::write(
            dir.join("boxes"),
            "192.168.122.40 mattbox.local\n192.168.122.40 other.local\n",
        )
        .unwrap();
        std::fs::write(dir.join(".boxes.partial"), "10.0.0.1 mattbox.local\n").unwrap();
        assert_eq!(
            find("MattBox.local.", &dir),
            Ok(Route {
                name: "mattbox.local".into(),
                address: "192.168.122.40".parse().unwrap(),
                machine: Some("boxes".into()),
            })
        );
        assert_eq!(offered(&dir), ["mattbox.local", "other.local"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn reads_the_template_pm_recorded() {
        let xml = "<domain>\n  <metadata>\n    <pm:machine xmlns:pm='https://losos.dasmat.us/pm'>\n      <pm:template name='losos-boxes' digest='0123abcd'/>\n    </pm:machine>\n  </metadata>\n</domain>";
        assert_eq!(machine_template(xml), Some("losos-boxes"));
        assert_eq!(
            machine_template("<template name=\"losos-guest\"/>"),
            Some("losos-guest")
        );
        assert_eq!(machine_template("<domain/>"), None);
    }

    #[test]
    fn spells_a_fingerprint_as_openssl_does() {
        let pem = "-----BEGIN CERTIFICATE-----\nAAEC\nAw==\n-----END CERTIFICATE-----\n";
        let der = pem_to_der(pem).unwrap();
        assert_eq!(der, [0, 1, 2, 3]);
        assert_eq!(
            fingerprint(&der),
            "05:4E:DE:C1:D0:21:1F:62:4F:ED:0C:BC:A9:D4:F9:40:0B:0E:49:1C:43:74:2A:F2:C5:B0:AB:EB:F0:C9:90:D8"
        );
    }
}
