//! An extension's `manifest.json`, read as Chrome and Firefox read it:
//! Manifest V2 and V3, `__MSG_name__` strings from `_locales`, and the
//! parts the runtime acts on (content scripts, the background, the
//! action, the options page, web-accessible resources, the content
//! security policy, commands and permissions). Everything else stays in
//! `raw`, which `chrome.runtime.getManifest()` hands the extension whole.
//!
//! Match patterns are kept as written; [`webkit_patterns`] turns them into
//! what WebKit's user script lists take, and [`pattern_matches`] decides
//! them here for web-accessible resources and host permissions.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// What the manifest could not be read as.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read manifest.json: {0}")]
    Read(#[from] std::io::Error),
    #[error("manifest.json is not JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest.json has no name")]
    NoName,
    #[error("manifest_version {0} is not 2 or 3")]
    Version(u64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct ContentScript {
    pub matches: Vec<String>,
    pub exclude_matches: Vec<String>,
    pub js: Vec<String>,
    pub css: Vec<String>,
    /// `document_start`, else at the end (`document_end` and
    /// `document_idle` alike).
    pub at_start: bool,
    pub all_frames: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Background {
    /// Manifest V2 scripts, run in a page made for them.
    Scripts(Vec<String>),
    /// A Manifest V2 page of the extension's own.
    Page(String),
    /// A Manifest V3 service worker, run in a page made for it; `module`
    /// loads it as an ES module.
    ServiceWorker { path: String, module: bool },
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Action {
    pub title: String,
    pub popup: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Accessible {
    pub resources: Vec<String>,
    pub matches: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Command {
    pub name: String,
    /// `Ctrl+Shift+L` and the like, as the manifest suggests it.
    pub key: Option<String>,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct Manifest {
    pub raw: Value,
    pub name: String,
    pub version: String,
    pub default_locale: Option<String>,
    pub permissions: Vec<String>,
    pub optional_permissions: Vec<String>,
    pub host_permissions: Vec<String>,
    pub content_scripts: Vec<ContentScript>,
    pub background: Option<Background>,
    pub action: Option<Action>,
    pub options_page: Option<String>,
    pub accessible: Vec<Accessible>,
    /// The content security policy the extension's pages are served
    /// with.
    pub csp: String,
    /// The policy and the pages the manifest sandboxes.
    pub sandbox_csp: String,
    pub sandbox_pages: Vec<String>,
    pub commands: Vec<Command>,
    pub key: Option<String>,
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

/// A match pattern, as opposed to a named permission such as `tabs`.
pub fn is_pattern(permission: &str) -> bool {
    permission == "<all_urls>" || permission.contains("://")
}

impl Manifest {
    /// Reads `dir/manifest.json`, with its strings in `lang` (then the
    /// manifest's default locale) where it has `_locales`.
    pub fn load(dir: &Path, lang: &str) -> Result<Self, Error> {
        let text = fs::read_to_string(dir.join("manifest.json"))?;
        // A UTF-8 byte order mark, which some packagers leave in.
        let raw: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))?;
        let default_locale = raw["default_locale"].as_str().map(str::to_owned);
        let locale = Locale::load(dir, lang, default_locale.as_deref());
        Self::parse(raw, &locale, default_locale)
    }

    fn parse(raw: Value, locale: &Locale, default_locale: Option<String>) -> Result<Self, Error> {
        let manifest_version = match raw["manifest_version"].as_u64() {
            Some(2) => 2,
            Some(3) => 3,
            other => return Err(Error::Version(other.unwrap_or(0))),
        };
        let text = |v: &Value| locale.substitute(v.as_str().unwrap_or_default());
        let name = text(&raw["name"]);
        if name.is_empty() {
            return Err(Error::NoName);
        }
        let mut permissions = strings(&raw["permissions"]);
        let mut host_permissions = strings(&raw["host_permissions"]);
        if manifest_version == 2 {
            // Manifest V2 lists hosts among the permissions.
            let (hosts, named): (Vec<String>, Vec<String>) =
                permissions.into_iter().partition(|p| is_pattern(p));
            permissions = named;
            host_permissions.extend(hosts);
        }
        let optional_permissions = strings(&raw["optional_permissions"])
            .into_iter()
            .chain(strings(&raw["optional_host_permissions"]))
            .collect();
        let content_scripts = raw["content_scripts"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|cs| ContentScript {
                        matches: strings(&cs["matches"]),
                        exclude_matches: strings(&cs["exclude_matches"]),
                        js: strings(&cs["js"]),
                        css: strings(&cs["css"]),
                        at_start: cs["run_at"].as_str() == Some("document_start"),
                        all_frames: cs["all_frames"].as_bool().unwrap_or(false),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let bg = &raw["background"];
        let background = if let Some(worker) = bg["service_worker"].as_str() {
            Some(Background::ServiceWorker {
                path: worker.to_owned(),
                module: bg["type"].as_str() == Some("module"),
            })
        } else if let Some(page) = bg["page"].as_str() {
            Some(Background::Page(page.to_owned()))
        } else if bg["scripts"].is_array() {
            Some(Background::Scripts(strings(&bg["scripts"])))
        } else {
            None
        };
        let action = [&raw["action"], &raw["browser_action"], &raw["page_action"]]
            .into_iter()
            .find(|a| a.is_object())
            .map(|a| Action {
                title: match text(&a["default_title"]) {
                    t if t.is_empty() => name.clone(),
                    t => t,
                },
                popup: a["default_popup"].as_str().filter(|p| !p.is_empty()).map(str::to_owned),
            });
        let options_page = raw["options_ui"]["page"]
            .as_str()
            .or(raw["options_page"].as_str())
            .filter(|p| !p.is_empty())
            .map(str::to_owned);
        let accessible = match &raw["web_accessible_resources"] {
            Value::Array(entries) if manifest_version == 3 => entries
                .iter()
                .map(|e| Accessible {
                    resources: strings(&e["resources"]),
                    matches: strings(&e["matches"]),
                })
                .collect(),
            // Manifest V2 lets every page load them.
            Value::Array(_) => vec![Accessible {
                resources: strings(&raw["web_accessible_resources"]),
                matches: vec!["<all_urls>".into()],
            }],
            _ => Vec::new(),
        };
        // Chrome's defaults, which also forbid remote code; the manifest's
        // own policy wins when it has one.
        let default_csp = "script-src 'self'; object-src 'self'";
        let (csp, sandbox_csp) = match &raw["content_security_policy"] {
            Value::String(s) => (s.clone(), String::new()),
            Value::Object(o) => (
                o.get("extension_pages")
                    .and_then(Value::as_str)
                    .unwrap_or(default_csp)
                    .to_owned(),
                o.get("sandbox").and_then(Value::as_str).unwrap_or_default().to_owned(),
            ),
            _ => (default_csp.to_owned(), String::new()),
        };
        let sandbox_csp = if sandbox_csp.is_empty() {
            "sandbox allow-scripts allow-forms allow-popups allow-modals; script-src 'self' 'unsafe-inline' 'unsafe-eval'; child-src 'self';".to_owned()
        } else {
            sandbox_csp
        };
        let sandbox_pages = strings(&raw["sandbox"]["pages"]);
        let commands = raw["commands"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(name, c)| Command {
                        name: name.clone(),
                        key: c["suggested_key"]["linux"]
                            .as_str()
                            .or(c["suggested_key"]["default"].as_str())
                            .map(str::to_owned),
                        description: text(&c["description"]),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Self {
            name,
            version: raw["version"].as_str().unwrap_or("0").to_owned(),
            default_locale,
            permissions,
            optional_permissions,
            host_permissions,
            content_scripts,
            background,
            action,
            options_page,
            accessible,
            csp,
            sandbox_csp,
            sandbox_pages,
            commands,
            key: raw["key"].as_str().map(str::to_owned),
            raw,
        })
    }

    /// The extension's id, as Chrome would give it: from its public key
    /// when the manifest carries one, so an extension keeps the id it has
    /// elsewhere; from its name otherwise.
    pub fn id(&self) -> String {
        let digest = match self.key.as_deref().and_then(base64_decode) {
            Some(key) => Sha256::digest(&key),
            None => Sha256::digest(self.name.as_bytes()),
        };
        // Chrome's "mpdecimal": the first 128 bits as hex, with each digit
        // moved to a-p so that the id is never mistaken for a number.
        digest[..16]
            .iter()
            .flat_map(|b| [b >> 4, b & 15])
            .map(|n| (b'a' + n) as char)
            .collect()
    }

    /// Whether `path` is one of the manifest's sandboxed pages.
    pub fn is_sandboxed(&self, path: &str) -> bool {
        self.sandbox_pages.iter().any(|p| p.trim_start_matches('/') == path)
    }

    /// Whether the extension asked for `permission` (`tabs`, `storage`,
    /// ...), as required or optional.
    pub fn has_permission(&self, permission: &str) -> bool {
        self.permissions.iter().any(|p| p == permission)
            || self.optional_permissions.iter().any(|p| p == permission)
    }

    /// Whether the extension may reach `url` from its own pages: a host
    /// permission matches it.
    pub fn allows_host(&self, url: &str) -> bool {
        self.host_permissions
            .iter()
            .chain(self.optional_permissions.iter().filter(|p| is_pattern(p)))
            .any(|p| pattern_matches(p, url))
    }

    /// Whether a page at `page_url` may load the extension's `path`
    /// (`web_accessible_resources`); the extension's own pages always may.
    pub fn is_accessible(&self, path: &str, page_url: &str) -> bool {
        self.accessible.iter().any(|entry| {
            entry.resources.iter().any(|r| glob_matches(r, path))
                && entry.matches.iter().any(|m| pattern_matches(m, page_url))
        })
    }

    /// Every path glob `web_accessible_resources` names, for any page.
    pub fn accessible_resources(&self) -> impl Iterator<Item = &str> {
        self.accessible.iter().flat_map(|entry| entry.resources.iter().map(String::as_str))
    }

    /// Every match pattern the content scripts cover, which the isolated
    /// world's api.js is injected on.
    pub fn content_script_patterns(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for cs in &self.content_scripts {
            for m in &cs.matches {
                if !out.contains(m) {
                    out.push(m.clone());
                }
            }
        }
        out
    }
}

/// The strings of one language, from `_locales/<lang>/messages.json`,
/// with the default locale's behind them.
#[derive(Debug, Default)]
pub struct Locale {
    messages: HashMap<String, Value>,
}

impl Locale {
    pub fn load(dir: &Path, lang: &str, default_locale: Option<&str>) -> Self {
        let mut messages = HashMap::new();
        // The default locale first, then the person's language over it:
        // `sk` falls back to `en` for what it lacks, and `pt_BR` tries
        // `pt` as Chrome does.
        let short = lang.split(['_', '-']).next().unwrap_or(lang);
        let mut chain: Vec<&str> = Vec::new();
        chain.extend(default_locale);
        chain.push(short);
        chain.push(lang);
        for code in chain {
            let path = dir.join("_locales").join(code.replace('-', "_")).join("messages.json");
            let Ok(text) = fs::read_to_string(path) else { continue };
            let Ok(Value::Object(map)) = serde_json::from_str::<Value>(text.trim_start_matches('\u{feff}')) else {
                continue;
            };
            for (name, entry) in map {
                messages.insert(name.to_lowercase(), entry);
            }
        }
        Self { messages }
    }

    /// `__MSG_name__` in `text` replaced by the name's message.
    pub fn substitute(&self, text: &str) -> String {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("__MSG_") {
            out.push_str(&rest[..start]);
            let after = &rest[start + 6..];
            match after.find("__") {
                Some(end) => {
                    let name = &after[..end];
                    match self.message(name) {
                        Some(message) => out.push_str(&message),
                        None => out.push_str(&rest[start..start + 6 + end + 2]),
                    }
                    rest = &after[end + 2..];
                }
                None => {
                    out.push_str(&rest[start..]);
                    rest = "";
                }
            }
        }
        out.push_str(rest);
        out
    }

    /// The message named `name`, with its placeholders' content in it
    /// (no substitutions given, as a manifest string has none).
    pub fn message(&self, name: &str) -> Option<String> {
        let entry = self.messages.get(&name.to_lowercase())?;
        let message = entry["message"].as_str()?;
        Some(expand_placeholders(message, &entry["placeholders"], &[]))
    }

    /// The messages as JSON, for `chrome.i18n` in the extension's pages.
    pub fn json(&self) -> String {
        serde_json::to_string(&self.messages).unwrap_or_else(|_| "{}".into())
    }
}

/// `$name$` placeholders replaced by their content, with `$1`..`$9` in
/// them taken from `substitutions`, as `chrome.i18n.getMessage` does.
pub fn expand_placeholders(message: &str, placeholders: &Value, substitutions: &[String]) -> String {
    let mut out = String::new();
    let mut rest = message;
    while let Some(start) = rest.find('$') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        if let Some(more) = after.strip_prefix('$') {
            out.push('$');
            rest = more;
            continue;
        }
        if let Some(digit) = after.chars().next().and_then(|c| c.to_digit(10)) {
            if digit > 0 {
                out.push_str(substitutions.get(digit as usize - 1).map_or("", String::as_str));
            }
            rest = &after[1..];
            continue;
        }
        match after.find('$') {
            Some(end) => {
                let name = &after[..end];
                let content = placeholders
                    .as_object()
                    .and_then(|o| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)))
                    .and_then(|(_, p)| p["content"].as_str());
                match content {
                    Some(content) => out.push_str(&expand_placeholders(content, &Value::Null, substitutions)),
                    None => out.push_str(&rest[start..start + 1 + end + 1]),
                }
                rest = &after[end + 1..];
            }
            None => {
                out.push('$');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(s.trim())
        .ok()
}

/// The parts of a match pattern: scheme, host and path, each possibly
/// `*`; `None` for one that is not a pattern.
fn split_pattern(pattern: &str) -> Option<(&str, &str, &str)> {
    if pattern == "<all_urls>" {
        return Some(("*", "*", "/*"));
    }
    let (scheme, rest) = pattern.split_once("://")?;
    let (host, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/*"),
    };
    Some((scheme, host, path))
}

/// Match patterns as WebKit's user content lists take them: the same
/// shape, less what WebKit refuses (`<all_urls>` spelt out, ports cut).
pub fn webkit_patterns(patterns: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for pattern in patterns {
        if pattern == "<all_urls>" {
            for p in ["*://*/*", "file:///*"] {
                if !out.iter().any(|o| o == p) {
                    out.push(p.to_owned());
                }
            }
            continue;
        }
        let Some((scheme, host, path)) = split_pattern(pattern) else {
            continue;
        };
        let host = host.split(':').next().unwrap_or(host);
        let p = format!("{scheme}://{host}{path}");
        if !out.contains(&p) {
            out.push(p);
        }
    }
    out
}

/// `*` in `glob` standing for any run of characters.
pub fn glob_matches(glob: &str, text: &str) -> bool {
    let mut parts = glob.split('*');
    let first = parts.next().unwrap_or_default();
    if !text.starts_with(first) {
        return false;
    }
    let mut rest = &text[first.len()..];
    let mut last: Option<&str> = None;
    for part in parts {
        last = Some(part);
        match rest.find(part) {
            Some(at) => rest = &rest[at + part.len()..],
            None => return false,
        }
    }
    match last {
        // A glob ending in `*` matches whatever is left.
        Some(part) => part.is_empty() || rest.is_empty(),
        None => rest.is_empty(),
    }
}

/// Whether `url` is in `pattern` (`<all_urls>`, `*://*.example.com/*`,
/// `https://example.com/path*`), as Chrome decides it.
pub fn pattern_matches(pattern: &str, url: &str) -> bool {
    let Some((p_scheme, p_host, p_path)) = split_pattern(pattern) else {
        return false;
    };
    let Some((scheme, rest)) = url.split_once("://") else {
        return false;
    };
    let scheme_ok = match p_scheme {
        "*" => scheme == "http" || scheme == "https",
        s => s.eq_ignore_ascii_case(scheme),
    };
    if !scheme_ok {
        return false;
    }
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    let p_host = p_host.split(':').next().unwrap_or(p_host).to_ascii_lowercase();
    let host_ok = if p_host == "*" {
        true
    } else if let Some(suffix) = p_host.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == p_host
    };
    host_ok && glob_matches(p_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(raw: Value) -> Manifest {
        Manifest::parse(raw, &Locale::default(), None).unwrap()
    }

    #[test]
    fn v2_hosts_come_from_permissions() {
        let m = parse(json!({
            "manifest_version": 2, "name": "T", "version": "1",
            "permissions": ["storage", "*://*.edupage.org/*", "<all_urls>"],
            "background": {"scripts": ["a.js", "b.js"], "persistent": false},
            "browser_action": {"default_title": "Open"},
            "web_accessible_resources": ["sidebar.html", "x/*.wasm"],
        }));
        assert_eq!(m.permissions, vec!["storage"]);
        assert_eq!(m.host_permissions, vec!["*://*.edupage.org/*", "<all_urls>"]);
        assert_eq!(m.background, Some(Background::Scripts(vec!["a.js".into(), "b.js".into()])));
        assert_eq!(m.action.as_ref().unwrap().title, "Open");
        assert!(m.is_accessible("sidebar.html", "https://x.edupage.org/a"));
        assert!(m.is_accessible("x/content_bg.wasm", "http://anything/"));
        assert!(!m.is_accessible("background.js", "https://x.edupage.org/"));
        assert!(m.allows_host("https://api.edupage.org/x?y"));
        assert_eq!(m.csp, "script-src 'self'; object-src 'self'");
    }

    #[test]
    fn v3_shape() {
        let m = parse(json!({
            "manifest_version": 3, "name": "T", "version": "1",
            "host_permissions": ["https://*/*"],
            "background": {"service_worker": "sw.js", "type": "module"},
            "action": {"default_popup": "popup.html"},
            "options_ui": {"page": "options.html"},
            "content_security_policy": {"extension_pages": "script-src 'self' 'wasm-unsafe-eval'; object-src 'self'"},
            "web_accessible_resources": [{"resources": ["menu.html"], "matches": ["<all_urls>"]}],
            "commands": {"_execute_action": {"suggested_key": {"default": "Ctrl+Shift+Y", "linux": "Ctrl+Shift+U"}}},
        }));
        assert_eq!(
            m.background,
            Some(Background::ServiceWorker { path: "sw.js".into(), module: true })
        );
        assert_eq!(m.action.as_ref().unwrap().popup.as_deref(), Some("popup.html"));
        assert_eq!(m.action.as_ref().unwrap().title, "T");
        assert_eq!(m.options_page.as_deref(), Some("options.html"));
        assert!(m.csp.contains("wasm-unsafe-eval"));
        assert!(m.is_accessible("menu.html", "http://a.example/"));
        assert!(!m.is_accessible("menu.html", "file:///tmp/x.html"));
        assert_eq!(m.commands[0].key.as_deref(), Some("Ctrl+Shift+U"));
        assert!(!m.allows_host("http://a.example/"));
        assert!(m.allows_host("https://a.example/"));
    }

    #[test]
    fn ids_are_chromes() {
        // Bitwarden's id in the Chrome Web Store comes from this key, the
        // one its .crx header carries.
        let m = parse(json!({"manifest_version": 3, "name": "Bitwarden", "version": "1",
            "key": "MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAmqKbvreshyXRuN2gikeR1idqR6KL0Di89JZcMyD4bjJRZVmQO7aznSGSALIHzSAUGYocUYBNDOP5QAhImxXyQ1qG8+goXs93v9GzrNJETdVuCEhqBggC4/DFabryJZDiKvZ2Jl0DM7MsWdoybZPwrj70V3aJ/nVNOMkf868scNTMliwitCqqjT5baTANsG0DkZWQExD4lSXzSZHH9MEO8q0iZ7RRlNuGRBAkZgNV8FwZRsPKm/rwQ9dy3VpgLcmLp5GiMt+kAEncqKAkuRYnhVXXBsKqIyYTMjHSLkLnpfFySyOPLBdS617i/PGNiP/MT6Xy6z//v5NozUgaAZ4gJQIDAQAB"}));
        assert_eq!(m.id(), "nngceckbapebfimnlniiiahkandclblb");
        let n = parse(json!({"manifest_version": 2, "name": "AIPage", "version": "1"}));
        assert_eq!(n.id().len(), 32);
        assert!(n.id().chars().all(|c| ('a'..='p').contains(&c)));
    }

    #[test]
    fn patterns() {
        assert!(pattern_matches("<all_urls>", "https://a.example/x"));
        assert!(pattern_matches("*://*.edupage.org/*", "https://login.edupage.org/"));
        assert!(pattern_matches("*://*.edupage.org/*", "http://edupage.org/x"));
        assert!(!pattern_matches("*://*.edupage.org/*", "https://notedupage.org/"));
        assert!(!pattern_matches("*://*/*", "ftp://a/"));
        assert!(pattern_matches("http://localhost:8080/*", "http://localhost:3000/x"));
        assert!(pattern_matches("https://a.example/path/*", "https://A.example/path/y?z"));
        assert!(!pattern_matches("https://a.example/path/*", "https://a.example/other"));
        assert!(pattern_matches("file:///*", "file:///tmp/x.html"));
        assert_eq!(
            webkit_patterns(&["<all_urls>".into(), "http://localhost:8080/*".into()]),
            vec!["*://*/*", "file:///*", "http://localhost/*"]
        );
        assert!(glob_matches("popup/fonts/*", "popup/fonts/a.woff"));
        assert!(glob_matches("*.wasm", "x/content_bg.wasm"));
        assert!(!glob_matches("a.js", "a.jsx"));
    }

    #[test]
    fn messages_substitute() {
        let dir = std::env::temp_dir().join(format!("danube-locale-{}", std::process::id()));
        fs::create_dir_all(dir.join("_locales/en")).unwrap();
        fs::create_dir_all(dir.join("_locales/sk")).unwrap();
        fs::write(
            dir.join("_locales/en/messages.json"),
            r#"{"extName": {"message": "Bitwarden"}, "hello": {"message": "Hi $NAME$, $1 $$", "placeholders": {"name": {"content": "$2"}}}}"#,
        )
        .unwrap();
        fs::write(dir.join("_locales/sk/messages.json"), r#"{"extName": {"message": "Bitwarden SK"}}"#).unwrap();
        let locale = Locale::load(&dir, "sk-SK", Some("en"));
        assert_eq!(locale.substitute("__MSG_extName__ x __MSG_missing__"), "Bitwarden SK x __MSG_missing__");
        assert_eq!(
            expand_placeholders("Hi $NAME$, $1 $$", &locale.messages["hello"]["placeholders"], &["a".into(), "b".into()]),
            "Hi b, a $"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
