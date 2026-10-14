//! A sandboxed extension page framed by a web page, with the extension's
//! own scripts and style sheets written into it.
//!
//! A sandboxed page has an opaque origin, and WebKit lets an opaque
//! document load nothing from a custom scheme unless the top page is of
//! that scheme too (CachedResourceLoader's
//! isNoCorsCrossOriginRequestToURLSchemeHandler). Chrome loads them, and
//! extensions frame such pages in web pages (Bitwarden's in-page menu is
//! one, sandboxed by its frame's own `sandbox` attribute, which no header
//! can lift). So the page arrives with its scripts and style sheets in it
//! and stays sandboxed; its content security policy allows the scripts by
//! hash. A style sheet its scripts add later, by address, is swapped for a
//! `data:` copy carried in the page: the style sheets beside it are.

use std::fs;
use std::path::Path;

use base64::Engine as _;
use sha2::{Digest, Sha256};

use super::store::safe_relative;

/// `html`, the page at `page` (a path in the extension at `dir`, whose
/// address starts with `base`), with each of the extension's own
/// `<script src>` and style sheet `<link>` replaced by its contents; and
/// the `'sha256-…'` sources its scripts need in `script-src`.
pub fn inline(html: &str, dir: &Path, base: &str, page: &str) -> (String, Vec<String>) {
    let mut out = String::with_capacity(html.len());
    let mut deferred = String::new();
    let mut hashes = Vec::new();
    let mut rest = html;
    let hash = |code: &str| format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(code.as_bytes())));
    // The swap goes before the page's own scripts, at the head's start.
    let mut swap = sheets_script(dir, base, page).map(|code| {
        hashes.push(hash(&code));
        format!("<script>{code}</script>")
    });
    while let Some(at) = rest.find('<') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[..=end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("<script") {
            let attrs = attributes(&tag[7..tag.len() - 1]);
            let close = rest[end + 1..].to_ascii_lowercase().find("</script>").map(|c| end + 1 + c + "</script>".len());
            let file = attr(&attrs, "src").and_then(|src| resolve(base, page, src)).and_then(|path| read(dir, &path));
            if let (Some(close), Some(code)) = (close, file) {
                let code = String::from_utf8_lossy(&code).replace("</script", "<\\/script").replace("</SCRIPT", "<\\/SCRIPT");
                hashes.push(hash(&code));
                let kind = attr(&attrs, "type").map(|t| format!(" type=\"{}\"", escape(t))).unwrap_or_default();
                let script = format!("<script{kind}>{code}</script>");
                // An inline script runs where it stands; one that was to
                // wait for the document (`defer`, or a module) goes last.
                let waits = attr(&attrs, "defer").is_some() || attr(&attrs, "type").is_some_and(|t| t.eq_ignore_ascii_case("module"));
                if waits {
                    deferred.push_str(&script);
                } else {
                    out.push_str(&script);
                }
                rest = &rest[close..];
                continue;
            }
        } else if lower.starts_with("<link") {
            let attrs = attributes(&tag[5..tag.len() - 1]);
            let sheet = attr(&attrs, "rel").is_some_and(|r| r.split_whitespace().any(|w| w.eq_ignore_ascii_case("stylesheet")));
            let file = attr(&attrs, "href").and_then(|href| resolve(base, page, href));
            if let (true, Some(path)) = (sheet, file) {
                if let Some(css) = read(dir, &path) {
                    out.push_str("<style>");
                    out.push_str(&urls(&String::from_utf8_lossy(&css), dir, base, &path));
                    out.push_str("</style>");
                    rest = &rest[end + 1..];
                    continue;
                }
            }
        } else if lower.starts_with("</body") {
            out.push_str(&deferred);
            deferred.clear();
        }
        out.push_str(tag);
        rest = &rest[end + 1..];
        if lower.starts_with("<head") {
            out.push_str(&swap.take().unwrap_or_default());
        }
    }
    out.push_str(rest);
    out.push_str(&deferred);
    // A page with no head of its own gets it at the end; its scripts that
    // ran earlier had no sheets to swap.
    out.push_str(&swap.unwrap_or_default());
    (out, hashes)
}

/// `csp` with `sources` added to the directive scripts are judged by.
pub fn allow_scripts(csp: &str, sources: &[String]) -> String {
    if sources.is_empty() {
        return csp.to_owned();
    }
    let directives: Vec<&str> = csp.split(';').map(str::trim).filter(|d| !d.is_empty()).collect();
    let name = |d: &str| d.split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
    // script-src if there is one, else default-src stands for it; with
    // neither, scripts are not restricted.
    let target = if directives.iter().any(|d| name(d) == "script-src") {
        "script-src"
    } else if directives.iter().any(|d| name(d) == "default-src") {
        "default-src"
    } else {
        return csp.to_owned();
    };
    directives
        .iter()
        .map(|d| if name(d) == target { format!("{d} {}", sources.join(" ")) } else { (*d).to_owned() })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A style sheet's `url()`s, which would not load in the opaque page:
/// each of the extension's own files becomes a `data:` address.
fn urls(css: &str, dir: &Path, base: &str, sheet: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(at) = rest.find("url(") {
        out.push_str(&rest[..at + 4]);
        rest = &rest[at + 4..];
        let Some(end) = rest.find(')') else { break };
        let reference = rest[..end].trim().trim_matches(['"', '\'']);
        let inlined = resolve(base, sheet, reference).and_then(|path| Some(data_url(super::runtime::content_type(&path), &read(dir, &path)?)));
        match inlined {
            Some(url) => out.push_str(&format!("\"{url}\"")),
            None => out.push_str(&rest[..end]),
        }
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

fn data_url(mime: &str, bytes: &[u8]) -> String {
    format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// The script that swaps a `<link>` to one of the style sheets beside
/// `page` for its `data:` copy, or `None` when there are none.
fn sheets_script(dir: &Path, base: &str, page: &str) -> Option<String> {
    let folder = page.rsplit_once('/').map(|(f, _)| f).unwrap_or("");
    let mut sheets = serde_json::Map::new();
    for entry in fs::read_dir(dir.join(folder)).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".css") || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = if folder.is_empty() { name } else { format!("{folder}/{name}") };
        let Some(css) = read(dir, &path) else { continue };
        let css = urls(&String::from_utf8_lossy(&css), dir, base, &path);
        sheets.insert(format!("{base}{path}"), data_url("text/css", css.as_bytes()).into());
    }
    if sheets.is_empty() {
        return None;
    }
    Some(format!(
        r#"(() => {{
  const sheets = {};
  const swap = (v) => {{
    try {{
      return sheets[new URL(String(v), location.href).href] || v;
    }} catch (e) {{
      return v;
    }}
  }};
  const set = Element.prototype.setAttribute;
  Element.prototype.setAttribute = function (name, value) {{
    if (this instanceof HTMLLinkElement && String(name).toLowerCase() === "href") value = swap(value);
    return set.call(this, name, value);
  }};
  const href = Object.getOwnPropertyDescriptor(HTMLLinkElement.prototype, "href");
  Object.defineProperty(HTMLLinkElement.prototype, "href", {{
    get: href.get,
    set(v) {{
      href.set.call(this, swap(v));
    }},
    configurable: true,
  }});
}})();"#,
        serde_json::Value::Object(sheets)
    ))
}

/// `reference`, as written in the file at `from`, as a path in the
/// extension; `None` for anything outside it (another site, `data:`).
fn resolve(base: &str, from: &str, reference: &str) -> Option<String> {
    let reference = reference.split(['?', '#']).next()?;
    let joined = if let Some(path) = reference.strip_prefix(base) {
        path.to_owned()
    } else if reference.contains(':') || reference.starts_with("//") || reference.is_empty() {
        return None;
    } else if let Some(path) = reference.strip_prefix('/') {
        path.to_owned()
    } else {
        let folder = from.rsplit_once('/').map(|(f, _)| f).unwrap_or("");
        format!("{folder}/{reference}")
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in joined.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

fn read(dir: &Path, path: &str) -> Option<Vec<u8>> {
    fs::read(dir.join(safe_relative(path).ok()?)).ok()
}

/// A tag's attributes as (lowercase name, value); a bare one has "".
fn attributes(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.trim_end_matches('/').char_indices().peekable();
    let s = s.trim_end_matches('/');
    while let Some(&(start, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut end = start;
        while let Some(&(i, c)) = chars.peek() {
            if c.is_whitespace() || c == '=' {
                break;
            }
            end = i + c.len_utf8();
            chars.next();
        }
        let name = s[start..end].to_ascii_lowercase();
        while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.peek().is_some_and(|&(_, c)| c == '=') {
            chars.next();
            while chars.peek().is_some_and(|&(_, c)| c.is_whitespace()) {
                chars.next();
            }
            match chars.peek().map(|&(_, c)| c) {
                Some(quote @ ('"' | '\'')) => {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == quote {
                            break;
                        }
                        value.push(c);
                    }
                }
                _ => {
                    while let Some(&(_, c)) = chars.peek() {
                        if c.is_whitespace() {
                            break;
                        }
                        value.push(c);
                        chars.next();
                    }
                }
            }
        }
        if !name.is_empty() {
            out.push((name, value));
        } else {
            chars.next();
        }
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_scripts_and_sheets_in() {
        let dir = std::env::temp_dir().join(format!("danube-inline-{}", std::process::id()));
        fs::create_dir_all(dir.join("overlay")).unwrap();
        fs::create_dir_all(dir.join("img")).unwrap();
        fs::write(dir.join("overlay/a.js"), "go('</script>')").unwrap();
        fs::write(dir.join("overlay/b.css"), "x{background:url(../img/p.png)} @font-face{src:url('../f.woff2')}").unwrap();
        fs::write(dir.join("img/p.png"), [1, 2, 3]).unwrap();
        let base = "chrome-extension://abc/";
        let html = r#"<html><head><script defer="defer" src="../overlay/a.js"></script><link href="b.css" rel="stylesheet"><script src="https://elsewhere/x.js"></script></head><body><p>hi</p></body></html>"#;
        let (page, hashes) = inline(html, &dir, base, "overlay/page.html");
        // The page's script and the swap of the sheets beside it.
        assert_eq!(hashes.len(), 2);
        assert!(page.starts_with("<html><head><script>(() => {"));
        // Deferred: after the body's content, escaped, and with its hash.
        assert!(page.contains("<p>hi</p><script>go('<\\/script>')</script></body>"));
        let code = "go('<\\/script>')";
        let expected = format!("'sha256-{}'", base64::engine::general_purpose::STANDARD.encode(Sha256::digest(code)));
        assert_eq!(hashes[1], expected);
        assert!(page.contains("url(\"data:image/png;base64,AQID\")"));
        // A file the extension lacks stays as written.
        assert!(page.contains("url('../f.woff2')"));
        assert!(page.contains("\"chrome-extension://abc/overlay/b.css\":\"data:text/css;base64,"));
        // Another site's script stays as it was.
        assert!(page.contains(r#"<script src="https://elsewhere/x.js"></script>"#));
        // The static <link> became a <style>.
        assert!(!page.contains("<link"));
    }

    #[test]
    fn hashes_join_script_src() {
        let sources = vec!["'sha256-x'".to_owned()];
        assert_eq!(allow_scripts("sandbox allow-scripts; script-src 'self'", &sources), "sandbox allow-scripts; script-src 'self' 'sha256-x'");
        assert_eq!(allow_scripts("default-src 'self'", &sources), "default-src 'self' 'sha256-x'");
        assert_eq!(allow_scripts("sandbox", &sources), "sandbox");
    }

    #[test]
    fn references_stay_in_the_extension() {
        assert_eq!(resolve("chrome-extension://a/", "o/p.html", "../x.js").as_deref(), Some("x.js"));
        assert_eq!(resolve("chrome-extension://a/", "o/p.html", "/y/x.js").as_deref(), Some("y/x.js"));
        assert_eq!(resolve("chrome-extension://a/", "o/p.html", "chrome-extension://a/z.js").as_deref(), Some("z.js"));
        assert_eq!(resolve("chrome-extension://a/", "o/p.html", "../../x.js"), None);
        assert_eq!(resolve("chrome-extension://a/", "o/p.html", "data:text/plain,x"), None);
    }
}
