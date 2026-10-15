//! The extension runtime: what makes an installed extension run. It lives
//! inside the engine (engine.rs), on WebKit's thread, and has four parts.
//!
//! The `chrome-extension://<id>/` scheme serves an extension's files, each
//! page with the manifest's content security policy, and the page the
//! background scripts or service worker run in, since WebKit has no worker
//! of its own to hand an extension.
//!
//! Content scripts are WebKit user scripts in an isolated world of the
//! extension's own, `ext-<id>`, preceded by api.js, which gives the world
//! `chrome.*`; its CSS is user style sheets. Extension pages (background,
//! popup, offscreen, an options page in a tab, an iframe in a web page) get
//! api.js in the page's own world, allowed only on the extension's origin.
//!
//! Every context api.js runs in registers itself with `hello` and then
//! sends each API call as one script message, answered here: messaging
//! and ports between contexts, tabs and windows, storage (JSON files under
//! `~/.local/share/danube/extension-data/<id>/`), scripting, alarms,
//! downloads, notifications, and the rest. What a context has to be told
//! (a message, a port, an event) waits here until the context asks with
//! `wait`, a script message the runtime answers only once there is
//! something: the one way to reach every frame of every world, since
//! WebKit's evaluate reaches only the main frame, and a web process
//! extension sees only the worlds it created itself.
//!
//! Each tab has a content manager of its own, so a script message says
//! which tab it came from; that is how a content script in any frame knows
//! its tab without the page being able to say otherwise.
//!
//! The background, the popup and an offscreen document are hidden web
//! views of their own; the popup's frames go to the window, which draws
//! them over the page.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Map, Value};
use tracing::{debug, info, trace, warn};

use super::inline;
use super::manifest::{self, Background, Manifest, pattern_matches, webkit_patterns};
use super::store::{Installed, Store};
use crate::engine::{emit, later, to_image, try_with, Engine, TabChange};
use crate::ffi::{WEBKIT_LOAD_COMMITTED, WEBKIT_LOAD_FINISHED, WEBKIT_LOAD_STARTED};
use crate::input::Input;
use crate::shared::{Command, Event, ExtCommand, ExtensionInfo, Shortcut, TabId};
use crate::webkit::{
    self, Buffer, Display, Frames, NetworkSession, Reply, URISchemeRequest,
    UserContentManager, UserScript, UserStyleSheet, View, WebContext, WebView,
};

const API: &str = include_str!("api.js");

/// The User-Agent an extension's pages and content scripts see: Chrome's,
/// since that is the browser whose paths extensions test, and whose API
/// this runtime provides.
const CHROME_UA: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";

/// Where a context runs, which decides what it receives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    /// A content script's isolated world in a tab's frame.
    Content,
    /// An extension page in a tab: an options page, or an iframe of the
    /// extension's in a web page.
    Tab,
    Background,
    Popup,
    Offscreen,
}

impl Kind {
    fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "content" => Kind::Content,
            "tab" => Kind::Tab,
            "background" => Kind::Background,
            "popup" => Kind::Popup,
            "offscreen" => Kind::Offscreen,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Content => "content",
            Kind::Tab => "tab",
            Kind::Background => "background",
            Kind::Popup => "popup",
            Kind::Offscreen => "offscreen",
        }
    }

    /// chrome.runtime.getContexts's name for it.
    fn context_type(self) -> &'static str {
        match self {
            Kind::Content | Kind::Tab => "TAB",
            Kind::Background => "BACKGROUND",
            Kind::Popup => "POPUP",
            Kind::Offscreen => "OFFSCREEN_DOCUMENT",
        }
    }
}

/// One place api.js runs: a frame of a page, in one world.
#[derive(Clone, Debug)]
struct Context {
    token: String,
    ext: String,
    kind: Kind,
    /// The hidden view it is in, if not a tab.
    view: Option<u64>,
    tab: Option<TabId>,
    /// Chrome's frame id: 0 for the main frame, a number of its own for
    /// each other context (worlds cannot see each other to agree on one).
    frame_id: u32,
    url: String,
}

/// A hidden web view of an extension's: its background, popup or
/// offscreen document.
struct ExtView {
    serial: u64,
    ext: String,
    kind: Kind,
    page: WebView,
    view: View,
    /// chrome.offscreen.createDocument's answer, owed once the page loads.
    waiting: Option<Reply>,
}

/// An extension as loaded: what was put where, to be taken out again.
struct Loaded {
    id: String,
    dir: PathBuf,
    manifest: Manifest,
    messages: String,
    world: String,
    /// What goes into every tab's content manager.
    scripts: Vec<UserScript>,
    sheets: Vec<UserStyleSheet>,
    /// chrome.scripting.registerContentScripts's, by their ids.
    registered: BTreeMap<String, (Value, Vec<UserScript>, Vec<UserStyleSheet>)>,
    background: Option<u64>,
    offscreen: Option<u64>,
    /// The background has run its scripts, so it is listening.
    ready: bool,
    /// Messages for the background sent before it was ready.
    queued: Vec<Queued>,
    badge: String,
    title: Option<String>,
    popup: Option<String>,
}

enum Queued {
    Message {
        id: u64,
        message: Value,
        sender: Value,
    },
    Connect {
        port: u64,
        name: String,
        sender: Value,
    },
}

/// A runtime.sendMessage waiting for its response.
struct Pending {
    reply: Option<Reply>,
    /// Contexts that have not said yet whether they handled it.
    outstanding: HashSet<String>,
    /// One said it will respond.
    handled: bool,
}

struct Port {
    creator: String,
    offered: Vec<String>,
    /// Contexts asked to accept that have not answered.
    outstanding: usize,
}

struct Alarm {
    scheduled: f64,
    period: Option<f64>,
    source: u32,
}

/// One storage area's items, written back a moment after they change.
struct Area {
    items: Map<String, Value>,
    path: Option<PathBuf>,
    dirty: bool,
}

pub struct Options {
    pub home: PathBuf,
    pub lang: String,
}

/// What runs once a context has taken a delivery: its answer, or the
/// error saying it went away first.
type Done = Box<dyn FnOnce(&mut Runtime, &mut Engine, Result<Value, String>)>;

pub struct Runtime {
    store: Store,
    lang: String,
    data: PathBuf,
    display: Display,
    session: NetworkSession,
    /// The extension views' settings: Chrome's User-Agent.
    settings: webkit::Settings,
    /// Each tab's content manager, which content scripts go into, and the
    /// worlds whose script message handler it has: a registration cannot
    /// be repeated, so it is made once per world and kept.
    managers: HashMap<TabId, (UserContentManager, HashSet<String>)>,
    /// Each tab's view, for the extension files its pages may load.
    pages: HashMap<TabId, WebView>,
    /// Kept for the scheme handler it holds.
    _context: WebContext,
    /// Proves a main-world context is an extension page: api.js carries it
    /// only where it is injected, the extension's own origin.
    secret: String,
    loaded: Vec<Loaded>,
    views: Vec<ExtView>,
    next_view: u64,
    contexts: HashMap<String, Context>,
    next_frame_id: u32,
    /// Each context's `wait`, held until there is something to hand it.
    waiters: HashMap<String, Reply>,
    /// What each context has yet to be handed.
    queues: HashMap<String, Vec<Value>>,
    /// Deliveries whose answer someone waits for, by sequence number, with
    /// the context they went to.
    acks: HashMap<u64, (String, Done)>,
    next_seq: u64,
    /// Answers owed by contexts that went away, run once the engine is
    /// free to be passed in (`with`).
    dropped: Vec<Done>,
    /// Contexts the last sweep found not waiting with something queued.
    stale: HashSet<String>,
    pending: HashMap<u64, Pending>,
    next_request: u64,
    ports: HashMap<u64, Port>,
    next_port: u64,
    alarms: HashMap<(String, String), Alarm>,
    storage: HashMap<(String, String), Area>,
    flush_source: Option<u32>,
    /// The file name an extension asked for a download by its URL.
    download_names: HashMap<String, String>,
    next_download: u64,
    next_notification: u64,
    popup: Option<u64>,
    first_views_before: bool,
}

/// Which content manager a script message came through.
#[derive(Clone, Copy)]
enum Origin {
    Tab(TabId),
    View(u64),
}

/// Runs `f` with the engine's runtime taken out, so both can be borrowed;
/// `None` when the engine has none. What happened to the tabs meanwhile
/// (`f` may open or close one) and the answers contexts that went away
/// still owe are dealt with before the runtime goes back.
pub fn with<R>(e: &mut Engine, f: impl FnOnce(&mut Runtime, &mut Engine) -> R) -> Option<R> {
    let mut rt = e.extensions.take()?;
    let result = f(&mut rt, e);
    loop {
        let changes = std::mem::take(&mut e.tab_changes);
        let dropped = std::mem::take(&mut rt.dropped);
        if changes.is_empty() && dropped.is_empty() {
            break;
        }
        for change in changes {
            rt.tab_change(e, change);
        }
        for done in dropped {
            done(&mut rt, e, Err("the context went away".into()));
        }
    }
    e.extensions = Some(rt);
    Some(result)
}

/// Tells the runtime what happened to a tab, now or, when the runtime is
/// busy (it opened or closed the tab itself), as soon as it is done.
pub fn notify(e: &mut Engine, change: TabChange) {
    if e.extensions.is_some() {
        with(e, |rt, e| rt.tab_change(e, change));
    } else {
        e.tab_changes.push(change);
    }
}

fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

/// 128 random bits as hex, for tokens a page must not guess.
fn random_token() -> String {
    let mut bytes = [0u8; 16];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = file.read_exact(&mut bytes);
    } else {
        let t = now_ms() as u128;
        bytes.copy_from_slice(&t.to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// `chrome-extension://<id>/<path>` taken apart; the query and fragment go.
fn split_uri(uri: &str) -> Option<(&str, &str)> {
    let rest = uri.strip_prefix("chrome-extension://")?;
    let rest = rest.split(['?', '#']).next()?;
    let (id, path) = rest.split_once('/').unwrap_or((rest, ""));
    Some((id, path))
}

/// An extension's sandboxed pages as WebKit URL patterns.
fn sandboxed(id: &str, manifest: &Manifest) -> Vec<String> {
    (manifest.sandbox_pages.iter())
        .map(|page| format!("chrome-extension://{id}/{}", page.trim_start_matches('/')))
        .collect()
}

pub(super) fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" | "htm" => "text/html",
        "js" | "mjs" => "text/javascript",
        "css" => "text/css",
        "json" => "application/json",
        "wasm" => "application/wasm",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "webm" => "video/webm",
        "mp4" => "video/mp4",
        "txt" | "map" => "text/plain",
        "xml" => "text/xml",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('"', "&quot;")
}

impl Runtime {
    /// Registers the scheme and loads every enabled extension. `display`,
    /// `session` and `content` are the engine's: the extension views share
    /// the first two, and content scripts go into the third.
    pub fn new(options: Options, display: &Display, session: &NetworkSession) -> Self {
        let context = WebContext::default();
        let security = context.security_manager();
        // Secure, so extension pages are secure contexts (crypto.subtle,
        // which a password manager lives on); CORS-enabled, so pages can
        // fetch web-accessible resources with the header the scheme sets.
        security.register_uri_scheme_as_secure("chrome-extension");
        security.register_uri_scheme_as_cors_enabled("chrome-extension");
        context.register_uri_scheme("chrome-extension", |request| {
            if try_with(|e| with(e, |rt, _| rt.serve(&request))).flatten().is_none() {
                request.finish_error("the extension runtime is busy");
            }
        });

        // An extension's pages may open windows. There are no developer
        // tools, so an extension's console is only seen here: on stdout,
        // when danube::extensions traces.
        let settings = webkit::Settings::new(true, tracing::enabled!(tracing::Level::TRACE));
        settings.set_user_agent(CHROME_UA);

        let mut rt = Self {
            store: Store::new(&options.home),
            lang: options.lang,
            data: options.home.join(".local/share/danube/extension-data"),
            display: display.clone(),
            session: session.clone(),
            settings,
            managers: HashMap::new(),
            pages: HashMap::new(),
            _context: context,
            secret: random_token(),
            loaded: Vec::new(),
            views: Vec::new(),
            next_view: 1,
            contexts: HashMap::new(),
            next_frame_id: 1,
            waiters: HashMap::new(),
            queues: HashMap::new(),
            acks: HashMap::new(),
            next_seq: 1,
            dropped: Vec::new(),
            stale: HashSet::new(),
            pending: HashMap::new(),
            next_request: 1,
            ports: HashMap::new(),
            next_port: 1,
            alarms: HashMap::new(),
            storage: HashMap::new(),
            flush_source: None,
            download_names: HashMap::new(),
            next_download: 1,
            next_notification: 1,
            popup: None,
            first_views_before: false,
        };
        rt.load_all();
        // A context has 10 to 20 seconds to come back for what waits for
        // it; an extension's page does at once unless it is gone.
        webkit::timeout_add(10_000, || {
            try_with(|e| with(e, |rt, _| rt.sweep()));
            true
        });
        rt
    }

    fn load_all(&mut self) {
        for installed in self.store.list(&self.lang) {
            if installed.enabled {
                self.load(installed);
            }
        }
        self.announce();
    }

    /// The window's and derisk's list of extensions.
    pub fn info(&self) -> Vec<ExtensionInfo> {
        let mut list: Vec<ExtensionInfo> = self
            .store
            .list(&self.lang)
            .into_iter()
            .map(|installed| {
                let loaded = self.loaded.iter().find(|l| l.id == installed.id);
                let action = installed.manifest.action.as_ref();
                let mut title = loaded
                    .and_then(|l| l.title.clone())
                    .or_else(|| action.map(|a| a.title.clone()).filter(|t| !t.is_empty()))
                    .unwrap_or_else(|| installed.manifest.name.clone());
                if let Some(badge) = loaded.map(|l| l.badge.as_str()).filter(|b| !b.is_empty()) {
                    title = format!("{title} ({badge})");
                }
                ExtensionInfo {
                    id: installed.id.clone(),
                    name: installed.manifest.name.clone(),
                    version: installed.manifest.version.clone(),
                    enabled: installed.enabled && loaded.is_some(),
                    title,
                    options: installed.manifest.options_page.is_some(),
                    commands: installed
                        .manifest
                        .commands
                        .iter()
                        .map(|c| Shortcut {
                            name: c.name.clone(),
                            key: c.key.clone(),
                            description: c.description.clone(),
                        })
                        .collect(),
                }
            })
            .collect();
        list.sort_by_key(|x| x.name.to_lowercase());
        list
    }

    fn announce(&self) {
        emit(Event::Extensions(self.info()));
    }

    fn loaded(&self, id: &str) -> Option<&Loaded> {
        self.loaded.iter().find(|l| l.id == id)
    }

    fn loaded_mut(&mut self, id: &str) -> Option<&mut Loaded> {
        self.loaded.iter_mut().find(|l| l.id == id)
    }

    fn view(&self, serial: u64) -> Option<&ExtView> {
        self.views.iter().find(|v| v.serial == serial)
    }

    /// api.js configured for one extension in one kind of context.
    fn api_source(&self, loaded: &Loaded, kind: Kind) -> String {
        let service_worker = matches!(
            loaded.manifest.background,
            Some(Background::ServiceWorker { .. })
        );
        let config = json!({
            "id": loaded.id,
            "manifest": loaded.manifest.raw,
            "messages": serde_json::from_str::<Value>(&loaded.messages).unwrap_or(Value::Object(Map::new())),
            "secret": if kind == Kind::Content { "" } else { self.secret.as_str() },
            "world": if kind == Kind::Content { loaded.world.as_str() } else { "" },
            "kind": kind.name(),
            "lang": self.lang,
            "platform": {"os": "linux", "arch": if cfg!(target_arch = "aarch64") { "arm64" } else { "x86-64" }, "nacl_arch": if cfg!(target_arch = "aarch64") { "arm64" } else { "x86-64" }},
            "ua": CHROME_UA,
            "version": env!("CARGO_PKG_VERSION"),
            "serviceWorker": service_worker,
        });
        API.replace("__DANUBE_CONFIG__", &config.to_string())
    }

    /// Registers the script message handler in `world` on the tabs'
    /// content manager, once per world: a registration cannot be taken
    /// back, and a second one of the same name is refused.
    /// Gives tab `tab`'s content manager the script message handler in
    /// `world` ("" for the page's own), once.
    fn ensure_world(&mut self, tab: TabId, world: &str) {
        let Some((manager, worlds)) = self.managers.get_mut(&tab) else { return };
        if !worlds.insert(world.to_owned()) {
            return;
        }
        let name = (!world.is_empty()).then_some(world);
        manager.register_message_handler("danube", name, move |text, reply| {
            message(Origin::Tab(tab), text, reply);
        });
    }

    /// Puts what `loaded` adds to pages into tab `tab`.
    fn install_into(&mut self, tab: TabId, at: usize) {
        let world = self.loaded[at].world.clone();
        self.ensure_world(tab, "");
        self.ensure_world(tab, &world);
        let Some((manager, _)) = self.managers.get(&tab) else { return };
        let loaded = &self.loaded[at];
        let registered = loaded.registered.values().flat_map(|(_, scripts, _)| scripts);
        for script in loaded.scripts.iter().chain(registered) {
            manager.add_script(script);
        }
        let registered = loaded.registered.values().flat_map(|(_, _, sheets)| sheets);
        for sheet in loaded.sheets.iter().chain(registered) {
            manager.add_style_sheet(sheet);
        }
    }

    /// Adds scripts and style sheets to every tab.
    fn add_everywhere(&mut self, scripts: &[UserScript], sheets: &[UserStyleSheet]) {
        for (manager, _) in self.managers.values() {
            for script in scripts {
                manager.add_script(script);
            }
            for sheet in sheets {
                manager.add_style_sheet(sheet);
            }
        }
    }

    fn remove_everywhere(&mut self, scripts: &[UserScript], sheets: &[UserStyleSheet]) {
        for (manager, _) in self.managers.values() {
            for script in scripts {
                manager.remove_script(script);
            }
            for sheet in sheets {
                manager.remove_style_sheet(sheet);
            }
        }
    }

    fn load(&mut self, installed: Installed) {
        let Installed {
            id, dir, manifest, ..
        } = installed;
        info!(extension = %id, name = %manifest.name, "loading an extension");
        let messages = manifest::Locale::load(&dir, &self.lang, manifest.default_locale.as_deref()).json();
        let world = format!("ext-{id}");
        let mut loaded = Loaded {
            id: id.clone(),
            dir: dir.clone(),
            manifest,
            messages,
            world: world.clone(),
            scripts: Vec::new(),
            sheets: Vec::new(),
            registered: BTreeMap::new(),
            background: None,
            offscreen: None,
            ready: false,
            queued: Vec::new(),
            badge: String::new(),
            title: None,
            popup: None,
        };

        // The isolated world, wherever the extension may run a script: its
        // content scripts' pages, and any page its host permissions let
        // chrome.scripting reach.
        let mut hosts = loaded.manifest.host_permissions.clone();
        hosts.extend(loaded.manifest.content_script_patterns());
        hosts.extend(
            loaded
                .manifest
                .optional_permissions
                .iter()
                .filter(|p| manifest::is_pattern(p))
                .cloned(),
        );
        // api.js goes first: WebKit runs a world's scripts in the order
        // they were added.
        let allow = webkit_patterns(&hosts);
        if !allow.is_empty() {
            let api = self.api_source(&loaded, Kind::Content);
            let script = UserScript::new(&api, Frames::All, true, Some(&world), &allow, &[]);
            loaded.scripts.push(script);
        }
        // The page's own world, on the extension's origin only, and not
        // in its sandboxed pages, which Chrome gives no extension API.
        let origin = vec![format!("chrome-extension://{id}/*")];
        let api = self.api_source(&loaded, Kind::Tab);
        let script = UserScript::new(&api, Frames::All, true, None, &origin, &sandboxed(&id, &loaded.manifest));
        loaded.scripts.push(script);

        for cs in loaded.manifest.content_scripts.clone() {
            let (scripts, sheets) = self.content_script(&loaded, &cs, None);
            loaded.scripts.extend(scripts);
            loaded.sheets.extend(sheets);
        }

        self.loaded.push(loaded);
        self.allow_accessible();
        let at = self.loaded.len() - 1;
        let tabs: Vec<TabId> = self.managers.keys().copied().collect();
        for tab in tabs {
            self.install_into(tab, at);
        }
        self.start_background(&id);
    }

    /// One content script entry as user scripts and style sheets, for the
    /// caller to add to the tabs. `main_world` runs the scripts in the
    /// page's own world instead (chrome.scripting's "MAIN").
    fn content_script(
        &self,
        loaded: &Loaded,
        cs: &manifest::ContentScript,
        main_world: Option<bool>,
    ) -> (Vec<UserScript>, Vec<UserStyleSheet>) {
        let allow = webkit_patterns(&cs.matches);
        let block = webkit_patterns(&cs.exclude_matches);
        let frames = if cs.all_frames {
            Frames::All
        } else {
            Frames::Top
        };
        let world = if main_world == Some(true) {
            None
        } else {
            Some(loaded.world.as_str())
        };
        let mut scripts = Vec::new();
        let mut sheets = Vec::new();
        for file in &cs.js {
            let path = loaded.dir.join(file.trim_start_matches('/'));
            let Ok(mut source) = std::fs::read_to_string(&path) else {
                warn!(extension = %loaded.id, file, "a content script is missing");
                continue;
            };
            // A source map comment would point WebKit at a file it cannot
            // fetch; the name helps a stack trace instead.
            source.push_str(&format!("\n//# sourceURL=chrome-extension://{}/{}", loaded.id, file));
            let script = UserScript::new(&source, frames, cs.at_start, world, &allow, &block);
            scripts.push(script);
        }
        for file in &cs.css {
            let path = loaded.dir.join(file.trim_start_matches('/'));
            let Ok(source) = std::fs::read_to_string(&path) else {
                warn!(extension = %loaded.id, file, "a content style sheet is missing");
                continue;
            };
            let sheet = UserStyleSheet::new(&source, frames, None, &allow, &block);
            sheets.push(sheet);
        }
        (scripts, sheets)
    }

    fn start_background(&mut self, id: &str) {
        let Some(loaded) = self.loaded(id) else { return };
        let url = match &loaded.manifest.background {
            None => return,
            Some(Background::Page(page)) => format!("chrome-extension://{id}/{}", page.trim_start_matches('/')),
            Some(_) => format!("chrome-extension://{id}/_generated_background_page.html"),
        };
        let serial = self.open_view(id, Kind::Background, &url);
        if let Some(loaded) = self.loaded_mut(id) {
            loaded.background = Some(serial);
        }
    }

    /// A hidden web view loading `url` for extension `id`; its serial.
    fn open_view(&mut self, id: &str, kind: Kind, url: &str) -> u64 {
        let serial = self.next_view;
        self.next_view += 1;
        let Some(loaded) = self.loaded(id) else { return serial };
        let content = UserContentManager::new();
        content.register_message_handler("danube", None, move |text, reply| {
            message(Origin::View(serial), text, reply);
        });
        let api = self.api_source(loaded, kind);
        let origin = vec![format!("chrome-extension://{id}/*")];
        let sandboxed = sandboxed(id, &loaded.manifest);
        content.add_script(&UserScript::new(&api, Frames::All, true, None, &origin, &sandboxed));
        let page = WebView::new(&self.display, &self.session, &self.settings, &content);
        // Its host permissions, without CORS: that is what they are for.
        page.set_cors_allowlist(&webkit_patterns(&loaded.manifest.host_permissions));
        let ext = id.to_owned();
        page.on_load_changed(move |_, event| {
            if try_with(|e| with(e, |rt, _| rt.view_load(serial, event))).is_none() {
                later(Command::Extensions(ExtCommand::ViewLoad(serial, event)));
            }
        });
        page.on_close(move |_| later(Command::Extensions(ExtCommand::ViewClosed(serial))));
        let name = ext.clone();
        page.on_web_process_terminated(move |_| {
            warn!(extension = %name, kind = kind.name(), "an extension's web process ended");
        });
        // An extension page may use the clipboard (a password manager
        // copies passwords); the rest stays denied, as a page it has no
        // person to ask.
        page.on_permission_request(|_, request| {
            if request.type_name().contains("Clipboard") {
                request.allow();
            } else {
                request.deny();
            }
            true
        });
        page.on_create(|_| None);
        let view = page.view();
        view.set_visible(false);
        page.load_uri(url);
        debug!(extension = %ext, kind = kind.name(), serial, url, "opened an extension view");
        self.views.push(ExtView {
            serial,
            ext,
            kind,
            page,
            view,
            waiting: None,
        });
        serial
    }

    fn close_view(&mut self, serial: u64) {
        let Some(at) = self.views.iter().position(|v| v.serial == serial) else {
            return;
        };
        let view = self.views.remove(at);
        if let Some(reply) = view.waiting {
            reply.error("the document was closed");
        }
        self.drop_contexts(|c| c.view == Some(serial));
        if let Some(loaded) = self.loaded_mut(&view.ext) {
            if loaded.background == Some(serial) {
                loaded.background = None;
            }
            if loaded.offscreen == Some(serial) {
                loaded.offscreen = None;
            }
        }
        if self.popup == Some(serial) {
            self.popup = None;
            emit(Event::PopupClosed);
        }
    }

    /// Lets every tab's pages load the extensions' web-accessible files.
    /// WebKit refuses a page's no-cors load into a custom scheme (a
    /// content script's `<script src>` or `<img>` of such a file) from any
    /// origin not allowed to reach it, and these patterns are that
    /// allowance; serve() still refuses what is not meant for the page
    /// asking. Other files stay out of a web page's reach, as in Chrome.
    fn allow_accessible(&self) {
        let patterns: Vec<String> = (self.loaded.iter())
            .flat_map(|l| l.manifest.accessible_resources().map(move |r| format!("chrome-extension://{}/{}", l.id, r.trim_start_matches('/'))))
            .collect();
        for page in self.pages.values() {
            page.set_cors_allowlist(&patterns);
        }
    }

    fn unload(&mut self, id: &str) {
        let Some(at) = self.loaded.iter().position(|l| l.id == id) else {
            return;
        };
        info!(extension = %id, "unloading an extension");
        self.flush_storage();
        let loaded = self.loaded.remove(at);
        self.allow_accessible();
        self.remove_everywhere(&loaded.scripts, &loaded.sheets);
        for (_, scripts, sheets) in loaded.registered.values() {
            self.remove_everywhere(scripts, sheets);
        }
        let serials: Vec<u64> = self.views.iter().filter(|v| v.ext == id).map(|v| v.serial).collect();
        for serial in serials {
            self.close_view(serial);
        }
        self.drop_contexts(|c| c.ext == id);
        let alarms: Vec<(String, String)> = self.alarms.keys().filter(|k| k.0 == id).cloned().collect();
        for key in alarms {
            if let Some(alarm) = self.alarms.remove(&key) {
                webkit::source_remove(alarm.source);
            }
        }
        self.storage.retain(|k, _| k.0 != id);
        for queued in loaded.queued {
            if let Queued::Message { id, .. } = queued {
                self.settle(id, None);
            }
        }
    }

    /// Loads, unloads or reloads what the store says: everything when
    /// `which` is `None`, else that one extension.
    fn reload(&mut self, which: Option<&str>) {
        let ids: Vec<String> = match which {
            Some(id) => vec![id.to_owned()],
            None => {
                let mut ids: Vec<String> = self.loaded.iter().map(|l| l.id.clone()).collect();
                ids.extend(self.store.list(&self.lang).into_iter().map(|i| i.id));
                ids.sort();
                ids.dedup();
                ids
            }
        };
        for id in ids {
            self.unload(&id);
            if let Some(installed) = self.store.list(&self.lang).into_iter().find(|i| i.id == id) {
                if installed.enabled {
                    self.load(installed);
                }
            }
        }
        self.announce();
    }

    // --- The scheme ----------------------------------------------------

    fn serve(&self, request: &URISchemeRequest) {
        let uri = request.uri();
        let Some((id, path)) = split_uri(&uri) else {
            request.finish_error("not an extension address");
            return;
        };
        let Some(loaded) = self.loaded(id) else {
            request.finish_error("no such extension");
            return;
        };
        let path = path.trim_start_matches('/');
        let prefix = format!("chrome-extension://{id}/");
        // The page in the view: an extension's own (a popup, an options
        // page in a tab) or a web page an extension's frame is in.
        let top = request.web_view().map(|v| v.uri()).unwrap_or_default();
        // Who asks: the document the request is for, by its Referer, else
        // the page in the view. An extension's own document may have
        // anything; a web page only what the manifest makes web-accessible
        // to it, as in Chrome.
        let referer = request.header("Referer").filter(|r| !r.is_empty());
        let asker = referer.clone().unwrap_or_else(|| top.clone());
        let own = asker.starts_with(&prefix);
        let accessible = own || loaded.manifest.is_accessible(path, &asker);
        let html = content_type(path) == "text/html";
        trace!(%uri, %asker, "an extension file was asked for");
        // WebKit sends a Referer for a web page's requests and none for an
        // extension document's, so a file is refused only when a web page
        // asks for it, and other files reach a web page only through the
        // patterns allow_accessible() sets. A page, which a frame in a web
        // page may load with no Referer, is judged by the tab's page too.
        if !accessible && (html || referer.is_some()) {
            debug!(%uri, %asker, "refused a file that is not web-accessible");
            request.finish(403, "text/plain", &[], b"This file is not web-accessible");
            return;
        }
        let mut headers: Vec<(String, String)> = Vec::new();
        if accessible && !own {
            headers.push(("Access-Control-Allow-Origin".into(), "*".into()));
        }
        if path == "_generated_background_page.html" {
            let csp = "script-src 'self' 'unsafe-eval' 'wasm-unsafe-eval' blob:; object-src 'self'";
            headers.push(("Content-Security-Policy".into(), csp.into()));
            request.finish(200, "text/html", &headers, self.background_page(loaded).as_bytes());
            return;
        }
        if path == "_danube_started.js" {
            request.finish(
                200,
                "text/javascript",
                &headers,
                b"if (globalThis.__danube && __danube.swStarted) __danube.swStarted();",
            );
            return;
        }
        let Ok(relative) = super::store::safe_relative(path) else {
            request.finish(404, "text/plain", &[], b"Not found");
            return;
        };
        let file = loaded.dir.join(relative);
        match std::fs::read(&file) {
            Ok(body) => {
                let mut body = body;
                if html {
                    let mut csp = if loaded.manifest.is_sandboxed(path) {
                        loaded.manifest.sandbox_csp.clone()
                    } else {
                        loaded.manifest.csp.clone()
                    };
                    // A sandboxed page in a web page could not load its own
                    // files (inline.rs says why), so they come in it.
                    if loaded.manifest.is_sandboxed(path) && !top.starts_with(&prefix) {
                        let (page, hashes) = inline::inline(&String::from_utf8_lossy(&body), &loaded.dir, &prefix, path);
                        csp = inline::allow_scripts(&csp, &hashes);
                        body = page.into_bytes();
                    }
                    headers.push(("Content-Security-Policy".into(), csp));
                }
                request.finish(200, content_type(path), &headers, &body);
            }
            Err(_) => {
                debug!(%uri, "an extension asked for a file it does not have");
                request.finish(404, "text/plain", &[], b"Not found");
            }
        }
    }

    /// The page a background's scripts (or service worker) run in.
    fn background_page(&self, loaded: &Loaded) -> String {
        let mut html = format!(
            "<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>{}</title></head><body>\n",
            escape_html(&loaded.manifest.name)
        );
        match &loaded.manifest.background {
            Some(Background::Scripts(scripts)) => {
                for script in scripts {
                    html.push_str(&format!("<script src=\"/{}\"></script>\n", escape_html(script.trim_start_matches('/'))));
                }
            }
            Some(Background::ServiceWorker { path, module }) => {
                let kind = if *module { " type=\"module\"" } else { "" };
                html.push_str(&format!("<script{kind} src=\"/{}\"></script>\n", escape_html(path.trim_start_matches('/'))));
                html.push_str("<script src=\"/_danube_started.js\"></script>\n");
            }
            _ => {}
        }
        html.push_str("</body></html>\n");
        html
    }

    // --- Contexts --------------------------------------------------------

    /// Registers the context `hello` describes; its answer.
    fn hello(&mut self, origin: Origin, args: &Value) -> Result<Value, String> {
        let info = &args[0];
        let ext = info["ext"].as_str().unwrap_or_default().to_owned();
        let world = info["world"].as_str().unwrap_or_default();
        let kind = Kind::from_name(info["kind"].as_str().unwrap_or_default()).ok_or("unknown context kind")?;
        let loaded = self.loaded(&ext).ok_or("no such extension")?;
        if !world.is_empty() {
            // An isolated world is reachable only by what was injected
            // into it, so it vouches for itself.
            if world != loaded.world || kind != Kind::Content {
                return Err("wrong world".into());
            }
        } else if info["secret"].as_str() != Some(self.secret.as_str()) {
            return Err("not an extension page".into());
        }
        let url = info["url"].as_str().unwrap_or_default().to_owned();
        let (view, tab, kind) = match origin {
            Origin::View(serial) => {
                let v = self.view(serial).ok_or("the view is gone")?;
                (Some(serial), None, v.kind)
            }
            Origin::Tab(tab) => (None, Some(tab), kind),
        };
        let frame_id = if info["top"].as_bool().unwrap_or(true) {
            0
        } else {
            self.next_frame_id += 1;
            self.next_frame_id
        };
        let token = random_token();
        let context = Context {
            token: token.clone(),
            ext,
            kind,
            view,
            tab,
            frame_id,
            url,
        };
        debug!(?context, "an extension context registered");
        self.contexts.insert(token.clone(), context);
        Ok(json!({"ctx": token, "tab": tab, "frameId": frame_id, "kind": kind.name()}))
    }

    fn drop_contexts(&mut self, which: impl Fn(&Context) -> bool) {
        let gone: Vec<String> = self.contexts.values().filter(|c| which(c)).map(|c| c.token.clone()).collect();
        for token in gone {
            self.drop_context(&token);
        }
    }

    fn drop_context(&mut self, token: &str) {
        let Some(context) = self.contexts.remove(token) else { return };
        debug!(?context, "an extension context went");
        // Its `wait` is not answered: the page it was in is gone or going.
        self.waiters.remove(token);
        self.queues.remove(token);
        // What was owed by it is owed no longer; the callers hear so once
        // the engine can be passed to them.
        let owed: Vec<u64> = self.acks.iter().filter(|(_, (t, _))| t == token).map(|(seq, _)| *seq).collect();
        for seq in owed {
            if let Some((_, done)) = self.acks.remove(&seq) {
                self.dropped.push(done);
            }
        }
        // Messages it was asked about are not coming.
        let ids: Vec<u64> = self.pending.iter().filter(|(_, p)| p.outstanding.contains(token)).map(|(id, _)| *id).collect();
        for id in ids {
            self.answered(id, token, false);
        }
        // Its ports close.
        let ports: Vec<u64> = self
            .ports
            .iter()
            .filter(|(_, p)| p.creator == token || p.offered.iter().any(|a| a == token))
            .map(|(id, _)| *id)
            .collect();
        for port in ports {
            self.disconnect(port, token);
        }
        // A background that reloads is not listening until it says so.
        if context.kind == Kind::Background && context.frame_id == 0 {
            if let Some(loaded) = self.loaded_mut(&context.ext) {
                loaded.ready = false;
            }
        }
    }

    /// The contexts of extension `ext` that `runtime.sendMessage` reaches:
    /// its pages, not its content scripts, and not the sender.
    fn extension_contexts(&self, ext: &str, except: &str) -> Vec<Context> {
        self.contexts
            .values()
            .filter(|c| c.ext == ext && c.kind != Kind::Content && c.token != except)
            .cloned()
            .collect()
    }

    /// The contexts in `tab` that `tabs.sendMessage` reaches: content
    /// scripts and extension frames, in one frame or all.
    fn tab_contexts(&self, ext: &str, tab: TabId, frame_id: Option<u32>) -> Vec<Context> {
        self.contexts
            .values()
            .filter(|c| c.ext == ext && c.tab == Some(tab) && frame_id.is_none_or(|f| c.frame_id == f))
            .cloned()
            .collect()
    }

    fn sender_json(&self, context: &Context, e: &Engine) -> Value {
        let mut sender = json!({"id": context.ext, "url": context.url, "origin": origin_of(&context.url)});
        if let Some(tab) = context.tab {
            if let Some(entry) = e.tabs.iter().find(|t| t.id == tab) {
                sender["tab"] = tab_json(entry, e.active == Some(tab));
                sender["frameId"] = json!(context.frame_id);
            }
        }
        sender["documentId"] = json!(context.token);
        sender
    }

    // --- Delivery ------------------------------------------------------

    /// Queues `item` for the context `token`, handing it over at once if
    /// the context is waiting; `done` gets the context's answer.
    fn deliver(&mut self, token: &str, mut item: Value, done: Option<Done>) {
        let seq = self.next_seq;
        self.next_seq += 1;
        item["seq"] = json!(seq);
        if let Some(done) = done {
            item["ack"] = json!(true);
            self.acks.insert(seq, (token.to_owned(), done));
        }
        self.queues.entry(token.to_owned()).or_default().push(item);
        self.flush(token);
    }

    /// Answers the context's `wait` with everything queued for it.
    fn flush(&mut self, token: &str) {
        if self.queues.get(token).is_none_or(Vec::is_empty) {
            return;
        }
        let Some(reply) = self.waiters.remove(token) else { return };
        let items = self.queues.remove(token).unwrap_or_default();
        reply.value(&Value::Array(items).to_string());
    }

    /// Fires `chrome.<ns>.<event>` in every page context of `ext` (and in
    /// its content scripts too for `all`).
    fn fire(&mut self, ext: &str, ns: &str, event: &str, args: Value, all: bool) {
        let item = json!({"t": "event", "ns": ns, "ev": event, "args": args});
        let tokens: Vec<String> = self
            .contexts
            .values()
            .filter(|c| c.ext == ext && (all || c.kind != Kind::Content))
            .map(|c| c.token.clone())
            .collect();
        for token in tokens {
            self.deliver(&token, item.clone(), None);
        }
    }

    fn fire_all(&mut self, ns: &str, event: &str, args: Value) {
        let ids: Vec<String> = self.loaded.iter().map(|l| l.id.clone()).collect();
        for id in ids {
            self.fire(&id, ns, event, args.clone(), false);
        }
    }

    /// Sends `message` from `sender` to `targets`, answering `reply` with
    /// the first response, or `null` when nobody responds.
    fn send_message(&mut self, ext: &str, message: Value, sender: Value, targets: Vec<Context>, reply: Reply) {
        if targets.is_empty() {
            // The background may still be starting; it gets the message
            // once it listens, as Chrome wakes a service worker for it.
            if self.loaded(ext).is_some_and(|l| l.background.is_some() && !l.ready) {
                let id = self.next_request;
                self.next_request += 1;
                self.pending.insert(id, Pending { reply: Some(reply), outstanding: HashSet::new(), handled: true });
                if let Some(loaded) = self.loaded_mut(ext) {
                    loaded.queued.push(Queued::Message { id, message, sender });
                }
                return;
            }
            reply.error("Could not establish connection. Receiving end does not exist.");
            return;
        }
        let id = self.next_request;
        self.next_request += 1;
        self.pending.insert(
            id,
            Pending {
                reply: Some(reply),
                outstanding: targets.iter().map(|c| c.token.clone()).collect(),
                handled: false,
            },
        );
        self.dispatch_message(id, message, sender, targets);
    }

    fn dispatch_message(&mut self, id: u64, message: Value, sender: Value, targets: Vec<Context>) {
        for context in targets {
            let item = json!({"t": "message", "id": id, "msg": message, "sender": sender});
            let token = context.token.clone();
            self.deliver(
                &context.token,
                item,
                Some(Box::new(move |rt, _, result| {
                    let handled = match result {
                        Ok(answer) => answer["handled"].as_bool().unwrap_or(false),
                        Err(error) => {
                            debug!(error, "a message could not be delivered");
                            false
                        }
                    };
                    rt.answered(id, &token, handled);
                })),
            );
        }
    }

    /// A target said whether it will respond to message `id`.
    fn answered(&mut self, id: u64, token: &str, handled: bool) {
        let Some(pending) = self.pending.get_mut(&id) else { return };
        pending.outstanding.remove(token);
        if handled {
            pending.handled = true;
        }
        if pending.outstanding.is_empty() && !pending.handled {
            self.settle(id, None);
        }
    }

    /// Answers message `id` with `response` (`null` for none).
    fn settle(&mut self, id: u64, response: Option<Value>) {
        if let Some(mut pending) = self.pending.remove(&id) {
            if let Some(reply) = pending.reply.take() {
                reply.value(&response.unwrap_or(Value::Null).to_string());
            }
        }
    }

    /// The background of `ext` is listening: what waited for it goes.
    fn background_ready(&mut self, ext: &str) {
        let queued = match self.loaded_mut(ext) {
            Some(loaded) => {
                loaded.ready = true;
                std::mem::take(&mut loaded.queued)
            }
            None => return,
        };
        let targets: Vec<Context> = self.contexts.values().filter(|c| c.ext == ext && c.kind == Kind::Background).cloned().collect();
        for item in queued {
            match item {
                Queued::Message { id, message, sender } => {
                    if targets.is_empty() {
                        self.settle(id, None);
                        continue;
                    }
                    if let Some(pending) = self.pending.get_mut(&id) {
                        pending.handled = false;
                        pending.outstanding = targets.iter().map(|c| c.token.clone()).collect();
                    }
                    self.dispatch_message(id, message, sender, targets.clone());
                }
                Queued::Connect { port, name, sender } => {
                    self.offer_port(port, name, sender, targets.clone());
                }
            }
        }
    }

    // --- Ports ------------------------------------------------------------

    fn connect(&mut self, e: &Engine, context: &Context, name: String, tab: Option<TabId>, frame_id: Option<u32>) -> u64 {
        let port = self.next_port;
        self.next_port += 1;
        self.ports.insert(
            port,
            Port {
                creator: context.token.clone(),
                offered: Vec::new(),
                outstanding: 0,
            },
        );
        let sender = self.sender_json(context, e);
        let targets = match tab {
            Some(tab) => self.tab_contexts(&context.ext, tab, frame_id),
            None => self.extension_contexts(&context.ext, &context.token),
        };
        if targets.is_empty() && tab.is_none() {
            if let Some(loaded) = self.loaded_mut(&context.ext) {
                if loaded.background.is_some() && !loaded.ready {
                    loaded.queued.push(Queued::Connect { port, name, sender });
                    return port;
                }
            }
        }
        self.offer_port(port, name, sender, targets);
        port
    }

    /// Offers port `port` to `targets`; those with an onConnect listener
    /// take it, and the creator hears when none did.
    fn offer_port(&mut self, port: u64, name: String, sender: Value, targets: Vec<Context>) {
        if targets.is_empty() {
            self.nobody_accepted(port);
            return;
        }
        // Everything offered the port hears what the creator posts until it
        // says it did not take it: a context gets its deliveries in order,
        // so the connect is there before the first message, and one with
        // no such port ignores the message.
        if let Some(p) = self.ports.get_mut(&port) {
            p.outstanding = targets.len();
            p.offered = targets.iter().map(|c| c.token.clone()).collect();
        }
        for context in targets {
            let item = json!({"t": "connect", "port": port, "name": name, "sender": sender});
            let token = context.token.clone();
            self.deliver(
                &context.token,
                item,
                Some(Box::new(move |rt, _, result| {
                    let accepted = result.ok().and_then(|v| v["accepted"].as_bool()).unwrap_or(false);
                    let Some(p) = rt.ports.get_mut(&port) else { return };
                    p.outstanding = p.outstanding.saturating_sub(1);
                    if !accepted {
                        p.offered.retain(|t| *t != token);
                    }
                    if p.outstanding == 0 && p.offered.is_empty() {
                        rt.nobody_accepted(port);
                    }
                })),
            );
        }
    }

    fn nobody_accepted(&mut self, port: u64) {
        let Some(p) = self.ports.remove(&port) else { return };
        if self.contexts.contains_key(&p.creator) {
            self.deliver(&p.creator, json!({"t": "disconnect", "port": port}), None);
        }
    }

    fn port_post(&mut self, port: u64, from: &str, message: Value) {
        let Some(p) = self.ports.get(&port) else { return };
        let item = json!({"t": "portmsg", "port": port, "msg": message});
        let targets: Vec<String> = if p.creator == from {
            p.offered.clone()
        } else {
            vec![p.creator.clone()]
        };
        for token in targets {
            if self.contexts.contains_key(&token) {
                self.deliver(&token, item.clone(), None);
            }
        }
    }

    /// `from` left port `port`: the other side hears when it is alone.
    fn disconnect(&mut self, port: u64, from: &str) {
        let Some(p) = self.ports.get_mut(&port) else { return };
        let item = json!({"t": "disconnect", "port": port});
        let tell: Vec<String> = if p.creator == from {
            let all = p.offered.clone();
            self.ports.remove(&port);
            all
        } else {
            p.offered.retain(|t| t != from);
            if p.offered.is_empty() && p.outstanding == 0 {
                let creator = p.creator.clone();
                self.ports.remove(&port);
                vec![creator]
            } else {
                Vec::new()
            }
        };
        for token in tell {
            if self.contexts.contains_key(&token) {
                self.deliver(&token, item.clone(), None);
            }
        }
    }

    // --- API calls ------------------------------------------------------

    /// Handles one script message: `hello`, or a call from a registered
    /// context.
    fn handle(&mut self, e: &mut Engine, origin: Origin, text: String, reply: Reply) {
        let Ok(request) = serde_json::from_str::<Value>(&text) else {
            reply.error("not JSON");
            return;
        };
        let method = request["m"].as_str().unwrap_or_default().to_owned();
        let args = request["a"].clone();
        if method == "hello" {
            match self.hello(origin, &args) {
                Ok(value) => reply.value(&value.to_string()),
                Err(error) => reply.error(&error),
            }
            return;
        }
        let Some(context) = request["c"].as_str().and_then(|c| self.contexts.get(c)).cloned() else {
            reply.error("unknown context");
            return;
        };
        match self.call(e, &context, &method, &args, reply) {
            Ok(Some(reply)) => reply.value("null"),
            Ok(None) => {}
            Err((reply, error)) => reply.error(&error),
        }
    }

    /// One API call. The reply is handed back when the call has no answer
    /// of its own (`Ok(Some)`), kept by what will answer later (`Ok(None)`),
    /// or returned with the error to reject it with.
    #[allow(clippy::too_many_lines)]
    fn call(&mut self, e: &mut Engine, ctx: &Context, method: &str, args: &Value, reply: Reply) -> Result<Option<Reply>, (Reply, String)> {
        let a = |n: usize| &args[n];
        let ext = ctx.ext.clone();
        macro_rules! value {
            ($v:expr) => {{
                let v: Value = $v;
                reply.value(&v.to_string());
                return Ok(None);
            }};
        }
        macro_rules! fail {
            ($msg:expr) => {
                return Err((reply, $msg.to_string()))
            };
        }
        macro_rules! tab_entry {
            ($id:expr) => {
                match e.tabs.iter().find(|t| t.id == $id) {
                    Some(entry) => entry,
                    None => fail!(format!("No tab with id: {}.", $id)),
                }
            };
        }
        match method {
            "wait" => {
                // A second wait from the same context replaces the first,
                // which goes back empty.
                if let Some(old) = self.waiters.insert(ctx.token.clone(), reply) {
                    old.value("[]");
                }
                self.flush(&ctx.token);
                Ok(None)
            }
            "ack" => {
                if let Some((_, done)) = a(0).as_u64().and_then(|seq| self.acks.remove(&seq)) {
                    done(self, e, Ok(a(1).clone()));
                }
                Ok(Some(reply))
            }
            "bye" => {
                self.drop_context(&ctx.token);
                Ok(Some(reply))
            }
            "ready" => {
                self.background_ready(&ext);
                self.first_run(&ext);
                Ok(Some(reply))
            }
            "respond" => {
                if let Some(id) = a(0).as_u64() {
                    self.settle(id, Some(a(1).clone()));
                }
                Ok(Some(reply))
            }
            "runtime.sendMessage" => {
                let sender = self.sender_json(ctx, e);
                let targets = self.extension_contexts(&ext, &ctx.token);
                self.send_message(&ext, a(0).clone(), sender, targets, reply);
                Ok(None)
            }
            "tabs.sendMessage" => {
                let Some(tab) = a(0).as_u64() else { fail!("tabs.sendMessage needs a tab id") };
                let frame = a(2)["frameId"].as_u64().map(|f| f as u32);
                let sender = self.sender_json(ctx, e);
                let targets = self.tab_contexts(&ext, tab, frame);
                if targets.is_empty() {
                    fail!("Could not establish connection. Receiving end does not exist.");
                }
                self.send_message(&ext, a(1).clone(), sender, targets, reply);
                Ok(None)
            }
            "runtime.connect" => {
                let name = a(0).as_str().unwrap_or_default().to_owned();
                let tab = a(1).as_u64();
                let frame = a(2).as_u64().map(|f| f as u32);
                let port = self.connect(e, ctx, name, tab, frame);
                value!(json!({"port": port}))
            }
            "port.post" => {
                if let Some(port) = a(0).as_u64() {
                    self.port_post(port, &ctx.token, a(1).clone());
                }
                Ok(Some(reply))
            }
            "port.disconnect" => {
                if let Some(port) = a(0).as_u64() {
                    self.disconnect(port, &ctx.token);
                }
                Ok(Some(reply))
            }
            "runtime.getContexts" => {
                let filter = a(0);
                let types: Option<Vec<&str>> = filter["contextTypes"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect());
                let list: Vec<Value> = self
                    .contexts
                    .values()
                    .filter(|c| c.ext == ext && c.kind != Kind::Content)
                    .filter(|c| types.as_ref().is_none_or(|t| t.contains(&c.kind.context_type())))
                    .map(|c| {
                        json!({
                            "contextType": c.kind.context_type(),
                            "contextId": c.token,
                            "tabId": c.tab.map_or(-1, |t| t as i64),
                            "windowId": if c.tab.is_some() { 1 } else { -1 },
                            "frameId": c.frame_id,
                            "documentId": c.token,
                            "documentUrl": c.url,
                            "documentOrigin": origin_of(&c.url),
                            "incognito": false,
                        })
                    })
                    .collect();
                value!(json!(list))
            }
            "runtime.openOptionsPage" => {
                self.open_options(e, &ext);
                Ok(Some(reply))
            }
            "runtime.reload" => {
                later(Command::Extensions(ExtCommand::Reload(Some(ext))));
                Ok(Some(reply))
            }
            "tabs.query" => {
                let q = a(0);
                let list: Vec<Value> = e
                    .tabs
                    .iter()
                    .filter(|t| q["active"].as_bool().is_none_or(|active| (e.active == Some(t.id)) == active))
                    .filter(|t| {
                        let patterns: Vec<String> = match &q["url"] {
                            Value::String(s) => vec![s.clone()],
                            Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                            _ => Vec::new(),
                        };
                        patterns.is_empty() || patterns.iter().any(|p| pattern_matches(p, &t.page.uri()))
                    })
                    .filter(|t| q["status"].as_str().is_none_or(|s| (s == "loading") == t.page.is_loading()))
                    .map(|t| tab_json(t, e.active == Some(t.id)))
                    .collect();
                value!(json!(list))
            }
            "tabs.get" => {
                let Some(id) = a(0).as_u64() else { fail!("tabs.get needs a tab id") };
                let entry = tab_entry!(id);
                value!(tab_json(entry, e.active == Some(id)))
            }
            "tabs.getCurrent" => match ctx.tab.and_then(|id| e.tabs.iter().find(|t| t.id == id)) {
                Some(entry) => value!(tab_json(entry, e.active == Some(entry.id))),
                None => value!(Value::Null),
            },
            "tabs.create" | "windows.create" => {
                let props = a(0);
                let url = props["url"].as_str().map(|u| self.absolute(&ext, u));
                let id = e.open(url.as_deref(), None);
                let active = props["active"].as_bool().unwrap_or(true) && props["focused"].as_bool().unwrap_or(true);
                if active {
                    e.activate(id);
                }
                let entry = tab_entry!(id);
                let tab = tab_json(entry, active);
                if method == "windows.create" {
                    value!(window_json(e, Some(1000 + id), Some(vec![tab])))
                }
                value!(tab)
            }
            "tabs.update" => {
                let Some(id) = a(0).as_u64().or(ctx.tab).or(e.active) else { fail!("no active tab") };
                let props = a(1);
                if let Some(url) = props["url"].as_str() {
                    let url = self.absolute(&ext, url);
                    tab_entry!(id).page.load_uri(&url);
                }
                if props["active"].as_bool() == Some(true) {
                    e.activate(id);
                }
                let entry = tab_entry!(id);
                value!(tab_json(entry, e.active == Some(id)))
            }
            "tabs.remove" => {
                let ids: Vec<u64> = match a(0) {
                    Value::Number(n) => n.as_u64().into_iter().collect(),
                    Value::Array(a) => a.iter().filter_map(Value::as_u64).collect(),
                    _ => Vec::new(),
                };
                for id in ids {
                    e.close(id);
                }
                Ok(Some(reply))
            }
            "tabs.reload" => {
                if let Some(entry) = a(0).as_u64().or(e.active).and_then(|id| e.tabs.iter().find(|t| t.id == id)) {
                    entry.page.reload();
                }
                Ok(Some(reply))
            }
            "tabs.goBack" | "tabs.goForward" => {
                if let Some(entry) = a(0).as_u64().or(e.active).and_then(|id| e.tabs.iter().find(|t| t.id == id)) {
                    if method == "tabs.goBack" {
                        entry.page.go_back();
                    } else {
                        entry.page.go_forward();
                    }
                }
                Ok(Some(reply))
            }
            "windows.get" | "windows.getCurrent" => {
                let id = a(0).as_i64();
                let populate = a(0)["populate"].as_bool().or(a(1)["populate"].as_bool()).unwrap_or(false);
                let tabs = populate.then(|| e.tabs.iter().map(|t| tab_json(t, e.active == Some(t.id))).collect());
                match id {
                    Some(id) if id >= 1000 => {
                        let tab = id as u64 - 1000;
                        let entry = tab_entry!(tab);
                        value!(window_json(e, Some(id as u64), populate.then(|| vec![tab_json(entry, e.active == Some(tab))])))
                    }
                    _ => value!(window_json(e, None, tabs)),
                }
            }
            "windows.getAll" => {
                let populate = a(0)["populate"].as_bool().unwrap_or(false);
                let tabs = populate.then(|| e.tabs.iter().map(|t| tab_json(t, e.active == Some(t.id))).collect());
                value!(json!([window_json(e, None, tabs)]))
            }
            "windows.update" => {
                let id = a(0).as_u64().unwrap_or(1);
                if id >= 1000 && a(1)["focused"].as_bool() == Some(true) {
                    e.activate(id - 1000);
                }
                value!(window_json(e, Some(id), None))
            }
            "windows.remove" => {
                if let Some(id) = a(0).as_u64().filter(|id| *id >= 1000) {
                    e.close(id - 1000);
                }
                Ok(Some(reply))
            }
            "storage.get" => {
                let area = a(0).as_str().unwrap_or("local").to_owned();
                let items = self.area(&ext, &area).items.clone();
                let out: Map<String, Value> = match a(1) {
                    Value::Null => items,
                    Value::String(key) => items.get(key).map(|v| [(key.clone(), v.clone())].into_iter().collect()).unwrap_or_default(),
                    Value::Array(keys) => keys.iter().filter_map(Value::as_str).filter_map(|k| items.get(k).map(|v| (k.to_owned(), v.clone()))).collect(),
                    Value::Object(defaults) => defaults.iter().map(|(k, d)| (k.clone(), items.get(k).cloned().unwrap_or(d.clone()))).collect(),
                    _ => Map::new(),
                };
                value!(Value::Object(out))
            }
            "storage.set" => {
                let area = a(0).as_str().unwrap_or("local").to_owned();
                let Value::Object(items) = a(1) else { fail!("storage.set takes an object") };
                let mut changes = Map::new();
                {
                    let store = self.area(&ext, &area);
                    for (k, v) in items {
                        let old = store.items.insert(k.clone(), v.clone());
                        if old.as_ref() != Some(v) {
                            let mut change = json!({"newValue": v});
                            if let Some(old) = old {
                                change["oldValue"] = old;
                            }
                            changes.insert(k.clone(), change);
                        }
                    }
                    store.dirty = true;
                }
                self.storage_changed(&ext, &area, changes);
                Ok(Some(reply))
            }
            "storage.remove" => {
                let area = a(0).as_str().unwrap_or("local").to_owned();
                let keys: Vec<String> = match a(1) {
                    Value::String(s) => vec![s.clone()],
                    Value::Array(a) => a.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    _ => Vec::new(),
                };
                let mut changes = Map::new();
                {
                    let store = self.area(&ext, &area);
                    for k in keys {
                        if let Some(old) = store.items.remove(&k) {
                            changes.insert(k, json!({"oldValue": old}));
                            store.dirty = true;
                        }
                    }
                }
                self.storage_changed(&ext, &area, changes);
                Ok(Some(reply))
            }
            "storage.clear" => {
                let area = a(0).as_str().unwrap_or("local").to_owned();
                let mut changes = Map::new();
                {
                    let store = self.area(&ext, &area);
                    for (k, old) in std::mem::take(&mut store.items) {
                        changes.insert(k, json!({"oldValue": old}));
                    }
                    store.dirty = true;
                }
                self.storage_changed(&ext, &area, changes);
                Ok(Some(reply))
            }
            "storage.getBytesInUse" => {
                let area = a(0).as_str().unwrap_or("local").to_owned();
                let items = &self.area(&ext, &area).items;
                let size: usize = match a(1) {
                    Value::String(k) => items.get(k).map_or(0, |v| k.len() + v.to_string().len()),
                    Value::Array(keys) => keys.iter().filter_map(Value::as_str).map(|k| items.get(k).map_or(0, |v| k.len() + v.to_string().len())).sum(),
                    _ => items.iter().map(|(k, v)| k.len() + v.to_string().len()).sum(),
                };
                value!(json!(size))
            }
            "scripting.executeScript" => {
                self.execute_script(e, ctx, a(0), reply);
                Ok(None)
            }
            "scripting.insertCSS" => {
                let injection = a(0);
                let mut css = injection["css"].as_str().unwrap_or_default().to_owned();
                if let Some(files) = injection["files"].as_array() {
                    let dir = self.loaded(&ext).map(|l| l.dir.clone()).unwrap_or_default();
                    for file in files.iter().filter_map(Value::as_str) {
                        css.push_str(&std::fs::read_to_string(dir.join(file.trim_start_matches('/'))).unwrap_or_default());
                        css.push('\n');
                    }
                }
                let script = format!(
                    "(function(){{var s=document.createElement('style');s.textContent={};(document.head||document.documentElement).appendChild(s);}})()",
                    json!(css)
                );
                let injection = json!({"target": injection["target"], "code": script});
                self.execute_script(e, ctx, &injection, reply);
                Ok(None)
            }
            "scripting.removeCSS" => Ok(Some(reply)),
            "scripting.registerContentScripts" => {
                let Some(list) = a(0).as_array() else { fail!("registerContentScripts takes a list") };
                for spec in list {
                    self.register_content_script(&ext, spec.clone());
                }
                Ok(Some(reply))
            }
            "scripting.updateContentScripts" => {
                let Some(list) = a(0).as_array() else { fail!("updateContentScripts takes a list") };
                for spec in list {
                    if let Some(id) = spec["id"].as_str() {
                        let merged = match self.loaded(&ext).and_then(|l| l.registered.get(id)) {
                            Some((old, _, _)) => {
                                let mut merged = old.clone();
                                if let (Value::Object(m), Value::Object(n)) = (&mut merged, spec) {
                                    for (k, v) in n {
                                        m.insert(k.clone(), v.clone());
                                    }
                                }
                                merged
                            }
                            None => spec.clone(),
                        };
                        self.unregister_content_script(&ext, id);
                        self.register_content_script(&ext, merged);
                    }
                }
                Ok(Some(reply))
            }
            "scripting.unregisterContentScripts" => {
                let ids: Vec<String> = match a(0)["ids"].as_array() {
                    Some(ids) => ids.iter().filter_map(Value::as_str).map(str::to_owned).collect(),
                    None => self.loaded(&ext).map(|l| l.registered.keys().cloned().collect()).unwrap_or_default(),
                };
                for id in ids {
                    self.unregister_content_script(&ext, &id);
                }
                Ok(Some(reply))
            }
            "scripting.getRegisteredContentScripts" => {
                let ids: Option<Vec<&str>> = a(0)["ids"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect());
                let list: Vec<Value> = self
                    .loaded(&ext)
                    .map(|l| l.registered.iter().filter(|(id, _)| ids.as_ref().is_none_or(|ids| ids.contains(&id.as_str()))).map(|(_, (spec, _, _))| spec.clone()).collect())
                    .unwrap_or_default();
                value!(json!(list))
            }
            "alarms.create" => {
                let name = a(0).as_str().unwrap_or_default().to_owned();
                let info = a(1);
                let now = now_ms();
                let when = info["when"].as_f64().unwrap_or_else(|| now + info["delayInMinutes"].as_f64().unwrap_or_else(|| info["periodInMinutes"].as_f64().unwrap_or(0.0)) * 60_000.0);
                let period = info["periodInMinutes"].as_f64();
                self.create_alarm(&ext, name, when, period);
                Ok(Some(reply))
            }
            "alarms.get" => {
                let name = a(0).as_str().unwrap_or_default();
                value!(self.alarms.get(&(ext.clone(), name.to_owned())).map_or(Value::Null, |al| alarm_json(name, al)))
            }
            "alarms.getAll" => {
                let list: Vec<Value> = self.alarms.iter().filter(|(k, _)| k.0 == ext).map(|(k, al)| alarm_json(&k.1, al)).collect();
                value!(json!(list))
            }
            "alarms.clear" => {
                let name = a(0).as_str().unwrap_or_default().to_owned();
                let had = self.alarms.remove(&(ext.clone(), name)).map(|al| webkit::source_remove(al.source)).is_some();
                value!(json!(had))
            }
            "alarms.clearAll" => {
                let keys: Vec<(String, String)> = self.alarms.keys().filter(|k| k.0 == ext).cloned().collect();
                let had = !keys.is_empty();
                for key in keys {
                    if let Some(al) = self.alarms.remove(&key) {
                        webkit::source_remove(al.source);
                    }
                }
                value!(json!(had))
            }
            "downloads.download" => {
                let options = a(0);
                let Some(url) = options["url"].as_str() else { fail!("downloads.download needs a url") };
                let page = match ctx.view {
                    Some(serial) => self.view(serial).map(|v| v.page.clone()),
                    None => ctx.tab.and_then(|id| e.tabs.iter().find(|t| t.id == id)).map(|t| t.page.clone()),
                };
                let Some(page) = page else { fail!("the context's page is gone") };
                if let Some(name) = options["filename"].as_str() {
                    self.download_names.insert(url.to_owned(), name.to_owned());
                }
                let id = self.next_download;
                self.next_download += 1;
                page.download_uri(url);
                let item = json!({"id": id, "url": url, "filename": options["filename"].as_str().unwrap_or(""), "state": "in_progress", "paused": false, "canResume": false, "bytesReceived": 0, "totalBytes": -1, "fileSize": -1, "exists": true, "incognito": false, "danger": "safe", "mime": "", "startTime": "", "referrer": ""});
                self.fire(&ext, "downloads", "onCreated", json!([item]), false);
                value!(json!(id))
            }
            "notifications.create" => {
                let options = a(1);
                let id = match a(0).as_str() {
                    Some(id) if !id.is_empty() => id.to_owned(),
                    _ => {
                        self.next_notification += 1;
                        format!("danube-{}", self.next_notification)
                    }
                };
                let title = options["title"].as_str().unwrap_or_default();
                let text = options["message"].as_str().unwrap_or_default();
                let name = self.loaded(&ext).map(|l| l.manifest.name.clone()).unwrap_or_default();
                emit(Event::Notice(if title.is_empty() { format!("{name}: {text}") } else { format!("{title}: {text}") }));
                value!(json!(id))
            }
            "commands.getAll" => {
                let list: Vec<Value> = self
                    .loaded(&ext)
                    .map(|l| l.manifest.commands.iter().map(|c| json!({"name": c.name, "description": c.description, "shortcut": c.key.clone().unwrap_or_default()})).collect())
                    .unwrap_or_default();
                value!(json!(list))
            }
            "action.setBadgeText" => {
                if let Some(loaded) = self.loaded_mut(&ext) {
                    loaded.badge = a(0)["text"].as_str().unwrap_or_default().to_owned();
                }
                self.announce();
                Ok(Some(reply))
            }
            "action.getBadgeText" => value!(json!(self.loaded(&ext).map(|l| l.badge.clone()).unwrap_or_default())),
            "action.setTitle" => {
                if let Some(loaded) = self.loaded_mut(&ext) {
                    loaded.title = a(0)["title"].as_str().map(str::to_owned);
                }
                self.announce();
                Ok(Some(reply))
            }
            "action.getTitle" => value!(json!(self.loaded(&ext).map(|l| l.title.clone().or_else(|| l.manifest.action.as_ref().map(|a| a.title.clone())).unwrap_or_default()))),
            "action.setPopup" => {
                if let Some(loaded) = self.loaded_mut(&ext) {
                    loaded.popup = a(0)["popup"].as_str().map(str::to_owned);
                }
                Ok(Some(reply))
            }
            "action.getPopup" => value!(json!(self.popup_path(&ext).unwrap_or_default())),
            "action.openPopup" => {
                self.open_popup(e, &ext);
                Ok(Some(reply))
            }
            "offscreen.createDocument" => {
                if self.loaded(&ext).and_then(|l| l.offscreen).is_some() {
                    fail!("Only a single offscreen document may be created.");
                }
                let Some(url) = a(0)["url"].as_str() else { fail!("offscreen.createDocument needs a url") };
                let url = self.absolute(&ext, url);
                let serial = self.open_view(&ext, Kind::Offscreen, &url);
                if let Some(loaded) = self.loaded_mut(&ext) {
                    loaded.offscreen = Some(serial);
                }
                if let Some(view) = self.views.iter_mut().find(|v| v.serial == serial) {
                    view.waiting = Some(reply);
                }
                Ok(None)
            }
            "offscreen.closeDocument" => {
                match self.loaded(&ext).and_then(|l| l.offscreen) {
                    Some(serial) => self.close_view(serial),
                    None => fail!("No current offscreen document."),
                }
                Ok(Some(reply))
            }
            "offscreen.hasDocument" => value!(json!(self.loaded(&ext).and_then(|l| l.offscreen).is_some())),
            "permissions.contains" | "permissions.request" => {
                let Some(loaded) = self.loaded(&ext) else { fail!("no such extension") };
                let perms = a(0)["permissions"].as_array().cloned().unwrap_or_default();
                let origins = a(0)["origins"].as_array().cloned().unwrap_or_default();
                let has = perms.iter().filter_map(Value::as_str).all(|p| loaded.manifest.has_permission(p))
                    && origins.iter().filter_map(Value::as_str).all(|o| {
                        loaded.manifest.host_permissions.iter().chain(loaded.manifest.optional_permissions.iter()).any(|h| h == o)
                            || loaded.manifest.allows_host(o)
                    });
                value!(json!(has))
            }
            "permissions.getAll" => {
                let Some(loaded) = self.loaded(&ext) else { fail!("no such extension") };
                value!(json!({"permissions": loaded.manifest.permissions, "origins": loaded.manifest.host_permissions}))
            }
            "webNavigation.getAllFrames" => {
                let Some(tab) = a(0)["tabId"].as_u64() else { fail!("getAllFrames needs a tab id") };
                value!(json!(self.frames_json(e, &ext, tab)))
            }
            "webNavigation.getFrame" => {
                let Some(tab) = a(0)["tabId"].as_u64() else { fail!("getFrame needs a tab id") };
                let frame = a(0)["frameId"].as_u64().unwrap_or(0);
                let found = self.frames_json(e, &ext, tab).into_iter().find(|f| f["frameId"].as_u64() == Some(frame));
                value!(found.unwrap_or(Value::Null))
            }
            _ => fail!(format!("{method} is not available in Danube")),
        }
    }

    /// A URL as an extension gives it: its own file by path, or absolute.
    fn absolute(&self, ext: &str, url: &str) -> String {
        if url.contains("://") || url.starts_with("about:") || url.starts_with("data:") || url.starts_with("blob:") {
            url.to_owned()
        } else {
            format!("chrome-extension://{ext}/{}", url.trim_start_matches('/'))
        }
    }

    fn frames_json(&self, e: &Engine, ext: &str, tab: TabId) -> Vec<Value> {
        let mut frames: Vec<Value> = Vec::new();
        if let Some(entry) = e.tabs.iter().find(|t| t.id == tab) {
            frames.push(json!({"frameId": 0, "parentFrameId": -1, "tabId": tab, "url": entry.page.uri(), "processId": 0, "errorOccurred": false, "documentId": format!("main-{tab}"), "frameType": "outermost_frame", "documentLifecycle": "active"}));
        }
        for c in self.contexts.values().filter(|c| c.ext == ext && c.tab == Some(tab) && c.frame_id != 0) {
            if frames.iter().any(|f| f["frameId"].as_u64() == Some(c.frame_id as u64)) {
                continue;
            }
            frames.push(json!({"frameId": c.frame_id, "parentFrameId": 0, "tabId": tab, "url": c.url, "processId": 0, "errorOccurred": false, "documentId": c.token, "frameType": "sub_frame", "documentLifecycle": "active"}));
        }
        frames
    }

    fn popup_path(&self, ext: &str) -> Option<String> {
        let loaded = self.loaded(ext)?;
        match &loaded.popup {
            Some(popup) => Some(popup.clone()).filter(|p| !p.is_empty()),
            None => loaded.manifest.action.as_ref().and_then(|a| a.popup.clone()).filter(|p| !p.is_empty()),
        }
    }

    fn open_options(&mut self, e: &mut Engine, ext: &str) {
        let Some(page) = self.loaded(ext).and_then(|l| l.manifest.options_page.clone()) else {
            return;
        };
        let url = self.absolute(ext, &page);
        if let Some(entry) = e.tabs.iter().find(|t| t.page.uri() == url) {
            let id = entry.id;
            e.activate(id);
            return;
        }
        let id = e.open(Some(&url), None);
        e.activate(id);
    }

    /// The toolbar button was pressed: the popup opens, or onClicked fires.
    fn action(&mut self, e: &mut Engine, ext: &str) {
        if self.popup_path(ext).is_some() {
            self.open_popup(e, ext);
        } else {
            let tab = e.active.and_then(|id| e.tabs.iter().find(|t| t.id == id)).map(|t| tab_json(t, true)).unwrap_or(Value::Null);
            let item = json!({"t": "command", "name": "_execute_action", "tab": tab});
            self.tell_pages(ext, &item);
        }
    }

    /// Delivers `item` to every page context of `ext`.
    fn tell_pages(&mut self, ext: &str, item: &Value) {
        let tokens: Vec<String> = self.contexts.values().filter(|c| c.ext == ext && c.kind != Kind::Content).map(|c| c.token.clone()).collect();
        for token in tokens {
            self.deliver(&token, item.clone(), None);
        }
    }

    fn shortcut(&mut self, e: &mut Engine, ext: &str, command: &str) {
        if command == "_execute_action" || command == "_execute_browser_action" || command == "_execute_page_action" {
            self.action(e, ext);
            return;
        }
        let tab = e.active.and_then(|id| e.tabs.iter().find(|t| t.id == id)).map(|t| tab_json(t, true)).unwrap_or(Value::Null);
        let item = json!({"t": "command", "name": command, "tab": tab});
        self.tell_pages(ext, &item);
    }

    // --- The popup --------------------------------------------------------

    /// The popup's size, in logical pixels: Chrome's largest.
    const POPUP: (i32, i32) = (400, 600);

    fn open_popup(&mut self, e: &mut Engine, ext: &str) {
        self.close_popup(e);
        let Some(path) = self.popup_path(ext) else { return };
        let url = self.absolute(ext, &path);
        let serial = self.open_view(ext, Kind::Popup, &url);
        self.popup = Some(serial);
        let Some(view) = self.view(serial) else { return };
        let (width, height) = Self::POPUP;
        let scale = e.size.2;
        if let Some(toplevel) = view.view.toplevel() {
            toplevel.set_scale(scale);
            toplevel.resize(width, height);
        }
        view.view.on_buffer_rendered(move |_, buffer| popup_frame(buffer));
        view.view.set_visible(true);
        e.set_focus(&view.view, true);
        if let Some(tab) = e.active.and_then(|id| e.view(id)) {
            e.set_focus(&tab, false);
        }
        emit(Event::PopupOpened {
            ext: ext.to_owned(),
            width,
            height,
        });
    }

    fn close_popup(&mut self, e: &mut Engine) {
        let Some(serial) = self.popup.take() else { return };
        self.close_view(serial);
        emit(Event::PopupClosed);
        if let Some(tab) = e.active.and_then(|id| e.view(id)) {
            e.set_focus(&tab, e.focused);
        }
    }

    fn popup_input(&mut self, e: &mut Engine, inputs: Vec<Input>) {
        let Some(view) = self.popup.and_then(|s| self.view(s)).map(|v| v.view.clone()) else {
            return;
        };
        for input in inputs {
            e.input(&view, input);
        }
    }

    // --- Scripting --------------------------------------------------------

    fn execute_script(&mut self, e: &Engine, ctx: &Context, injection: &Value, reply: Reply) {
        let ext = ctx.ext.clone();
        let target = &injection["target"];
        let Some(tab) = target["tabId"].as_u64().or(ctx.tab) else {
            reply.error("executeScript needs a tab id");
            return;
        };
        let Some(page) = e.tabs.iter().find(|t| t.id == tab).map(|t| t.page.clone()) else {
            reply.error(&format!("No tab with id: {tab}."));
            return;
        };
        let main_world = injection["world"].as_str() == Some("MAIN");
        let Some(loaded) = self.loaded(&ext) else {
            reply.error("no such extension");
            return;
        };
        let mut source = String::new();
        if let Some(files) = injection["files"].as_array() {
            for file in files.iter().filter_map(Value::as_str) {
                match std::fs::read_to_string(loaded.dir.join(file.trim_start_matches('/'))) {
                    Ok(text) => {
                        source.push_str(&text);
                        source.push_str(&format!("\n//# sourceURL=chrome-extension://{ext}/{file}\n"));
                    }
                    Err(_) => {
                        reply.error(&format!("Could not load file: '{file}'."));
                        return;
                    }
                }
            }
        } else if let Some(func) = injection["func"].as_str() {
            let args: Vec<String> = injection["args"].as_array().map(|a| a.iter().map(Value::to_string).collect()).unwrap_or_default();
            source = format!("({func})({})", args.join(","));
        } else if let Some(code) = injection["code"].as_str() {
            source = code.to_owned();
        }
        let world = (!main_world).then(|| loaded.world.clone());
        let api = self.api_source(loaded, Kind::Content);

        // Which frames: the ones named, or the main one and, for allFrames,
        // every frame the extension's world has reached.
        let content_in = |frame: u32| {
            self.contexts
                .values()
                .find(|c| c.ext == ext && c.tab == Some(tab) && c.frame_id == frame && c.kind == Kind::Content)
                .map(|c| c.token.clone())
        };
        let mut frames: Vec<u32> = match target["frameIds"].as_array() {
            Some(ids) => ids.iter().filter_map(Value::as_u64).map(|id| id as u32).collect(),
            None => {
                let mut frames = vec![0];
                if target["allFrames"].as_bool() == Some(true) {
                    frames.extend(self.contexts.values().filter(|c| c.ext == ext && c.tab == Some(tab) && c.kind == Kind::Content).map(|c| c.frame_id));
                }
                frames
            }
        };
        frames.sort_unstable();
        frames.dedup();
        let plan: Vec<(u32, Option<String>)> = frames.into_iter().map(|f| (f, (!main_world).then(|| content_in(f)).flatten())).collect();

        let gather = Rc::new(RefCell::new(Gather { results: Vec::new(), left: plan.len(), reply: Some(reply) }));
        for (frame_id, token) in plan {
            let gather = gather.clone();
            let finish = move |result: Result<Value, String>| {
                let value = result.unwrap_or_else(|error| {
                    debug!(error, frame_id, "executeScript failed in a frame");
                    Value::Null
                });
                gather.borrow_mut().add(json!({"frameId": frame_id, "result": value, "documentId": ""}));
            };
            match token {
                // The extension's world is already in that frame: it runs
                // the code itself, and awaits what it returns.
                Some(token) => self.deliver(
                    &token,
                    json!({"t": "exec", "code": source}),
                    Some(Box::new(move |_, _, result| finish(result.map(|v| v["value"].clone())))),
                ),
                // The main frame through WebKit, with chrome.* first where
                // the extension's world has not run yet.
                None if frame_id == 0 => {
                    let script = if main_world { source.clone() } else { format!("{api}\n{source}") };
                    page.evaluate(
                        &script,
                        world.as_deref(),
                        Some(Box::new(move |result| finish(result.map(|json| serde_json::from_str(&json).unwrap_or(Value::Null))))),
                    );
                }
                None => finish(Err("the extension's world has not reached that frame".into())),
            }
        }
    }

    fn register_content_script(&mut self, ext: &str, spec: Value) {
        let Some(id) = spec["id"].as_str().map(str::to_owned) else { return };
        let strings = |v: &Value| v.as_array().map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect::<Vec<_>>()).unwrap_or_default();
        let cs = manifest::ContentScript {
            matches: strings(&spec["matches"]),
            exclude_matches: strings(&spec["excludeMatches"]),
            js: strings(&spec["js"]),
            css: strings(&spec["css"]),
            at_start: spec["runAt"].as_str() == Some("document_start"),
            all_frames: spec["allFrames"].as_bool().unwrap_or(false),
        };
        let main_world = spec["world"].as_str() == Some("MAIN");
        let Some(loaded) = self.loaded(ext) else { return };
        let (scripts, sheets) = self.content_script(loaded, &cs, Some(main_world));
        self.add_everywhere(&scripts, &sheets);
        if let Some(loaded) = self.loaded_mut(ext) {
            loaded.registered.insert(id, (spec, scripts, sheets));
        }
    }

    fn unregister_content_script(&mut self, ext: &str, id: &str) {
        let Some(loaded) = self.loaded_mut(ext) else { return };
        if let Some((_, scripts, sheets)) = loaded.registered.remove(id) {
            self.remove_everywhere(&scripts, &sheets);
        }
    }

    // --- Storage ---------------------------------------------------------

    fn area(&mut self, ext: &str, area: &str) -> &mut Area {
        let key = (ext.to_owned(), area.to_owned());
        if !self.storage.contains_key(&key) {
            let path = match area {
                // Nothing to sync with: `sync` is a second local file.
                "local" | "sync" => Some(self.data.join(ext).join(format!("{area}.json"))),
                _ => None,
            };
            let items = path
                .as_ref()
                .and_then(|p| std::fs::read_to_string(p).ok())
                .and_then(|text| serde_json::from_str::<Value>(&text).ok())
                .and_then(|v| match v {
                    Value::Object(m) => Some(m),
                    _ => None,
                })
                .unwrap_or_default();
            self.storage.insert(key.clone(), Area { items, path, dirty: false });
        }
        self.storage.get_mut(&key).expect("just inserted")
    }

    fn storage_changed(&mut self, ext: &str, area: &str, changes: Map<String, Value>) {
        if changes.is_empty() {
            return;
        }
        self.fire(ext, "storage", "onChanged", json!([changes, area]), true);
        // Writes coalesce: a page sets several keys in a row.
        if self.flush_source.is_none() {
            self.flush_source = Some(webkit::timeout_add(200, || {
                try_with(|e| {
                    with(e, |rt, _| {
                        rt.flush_source = None;
                        rt.flush_storage();
                    })
                });
                false
            }));
        }
    }

    /// Writes every changed area now.
    pub fn flush_storage(&mut self) {
        for area in self.storage.values_mut() {
            if !area.dirty {
                continue;
            }
            area.dirty = false;
            let Some(path) = &area.path else { continue };
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let temp = path.with_extension("json.tmp");
            let text = Value::Object(area.items.clone()).to_string();
            if std::fs::write(&temp, text).and_then(|()| std::fs::rename(&temp, path)).is_err() {
                warn!(path = %path.display(), "cannot write an extension's storage");
            }
        }
    }

    // --- Alarms -------------------------------------------------------

    fn create_alarm(&mut self, ext: &str, name: String, when: f64, period: Option<f64>) {
        let key = (ext.to_owned(), name.clone());
        if let Some(old) = self.alarms.remove(&key) {
            webkit::source_remove(old.source);
        }
        let delay = (when - now_ms()).max(0.0) as u32;
        let (ext2, name2) = (ext.to_owned(), name.clone());
        let source = webkit::timeout_add(delay, move || {
            let (ext, name) = (ext2.clone(), name2.clone());
            try_with(|e| with(e, |rt, _| rt.alarm_fired(&ext, &name)));
            false
        });
        self.alarms.insert(key, Alarm { scheduled: when, period, source });
    }

    fn alarm_fired(&mut self, ext: &str, name: &str) {
        let key = (ext.to_owned(), name.to_owned());
        let Some(alarm) = self.alarms.remove(&key) else { return };
        let info = alarm_json(name, &alarm);
        if let Some(period) = alarm.period {
            self.create_alarm(ext, name.to_owned(), now_ms() + period * 60_000.0, Some(period));
        }
        self.fire(ext, "alarms", "onAlarm", json!([info]), false);
    }

    // --- What the engine tells the runtime -------------------------------

    /// A load event in tab `id`.
    pub fn tab_load(&mut self, e: &Engine, id: TabId, event: u32) {
        let Some(entry) = e.tabs.iter().find(|t| t.id == id) else { return };
        let url = entry.page.uri();
        let time = now_ms();
        // The old document's contexts say `bye` as it goes, and the sweep
        // finds those that could not; dropping them here could drop the new
        // document's too, should this event come late.
        if event == WEBKIT_LOAD_STARTED {
            self.fire_all("webNavigation", "onBeforeNavigate", json!([{"tabId": id, "url": url, "frameId": 0, "parentFrameId": -1, "timeStamp": time, "processId": 0, "documentLifecycle": "active", "frameType": "outermost_frame"}]));
            self.fire_all("tabs", "onUpdated", json!([id, {"status": "loading", "url": url}, tab_json(entry, e.active == Some(id))]));
        } else if event == WEBKIT_LOAD_COMMITTED {
            self.fire_all("webNavigation", "onCommitted", json!([{"tabId": id, "url": url, "frameId": 0, "parentFrameId": -1, "timeStamp": time, "processId": 0, "transitionType": "link", "transitionQualifiers": [], "documentLifecycle": "active", "frameType": "outermost_frame"}]));
        } else if event == WEBKIT_LOAD_FINISHED {
            let details = json!({"tabId": id, "url": url, "frameId": 0, "parentFrameId": -1, "timeStamp": time, "processId": 0, "documentLifecycle": "active", "frameType": "outermost_frame"});
            self.fire_all("webNavigation", "onDOMContentLoaded", json!([details]));
            self.fire_all("webNavigation", "onCompleted", json!([details]));
            self.fire_all("tabs", "onUpdated", json!([id, {"status": "complete", "url": url, "title": entry.page.title()}, tab_json(entry, e.active == Some(id))]));
        }
    }

    /// A tab opened, closed or came to the front.
    fn tab_change(&mut self, e: &Engine, change: TabChange) {
        match change {
            TabChange::Opened(id) => {
                let Some(entry) = e.tabs.iter().find(|t| t.id == id) else { return };
                self.managers.insert(id, (entry.content.clone(), HashSet::new()));
                self.pages.insert(id, entry.page.clone());
                self.allow_accessible();
                self.ensure_world(id, "");
                for at in 0..self.loaded.len() {
                    self.install_into(id, at);
                }
                self.fire_all("tabs", "onCreated", json!([tab_json(entry, false)]));
            }
            TabChange::Closed(id) => {
                self.managers.remove(&id);
                self.pages.remove(&id);
                self.drop_contexts(|c| c.tab == Some(id));
                self.fire_all("tabs", "onRemoved", json!([id, {"windowId": 1, "isWindowClosing": false}]));
            }
            TabChange::Activated(id) => {
                self.fire_all("tabs", "onActivated", json!([{"tabId": id, "windowId": 1}]));
            }
        }
    }

    /// Drops the contexts that have had something waiting for them, and
    /// no `wait` of theirs held, since the last sweep: their page went
    /// without saying `bye` (it crashed, or was torn down mid-script), and
    /// a message to them would otherwise never be answered.
    ///
    /// A page busy for longer (deriving a key, say) looks the same, so
    /// api.js registers again if it finds itself dropped. The contexts of
    /// extension views are never swept: their view's load and close
    /// events say when they go, and a background busy that long would
    /// lose what was sent to it.
    fn sweep(&mut self) {
        let waiting: HashSet<String> = self
            .queues
            .iter()
            .filter(|(token, queue)| !queue.is_empty() && !self.waiters.contains_key(*token))
            .filter(|(token, _)| self.contexts.get(*token).is_some_and(|c| c.view.is_none()))
            .map(|(token, _)| token.clone())
            .collect();
        let gone: Vec<String> = waiting.intersection(&self.stale).cloned().collect();
        for token in gone {
            debug!(token, "an extension context stopped answering");
            self.drop_context(&token);
        }
        self.stale = waiting;
    }

    /// A load event in extension view `serial`.
    fn view_load(&mut self, serial: u64, event: u32) {
        trace!(serial, event, "an extension view's load changed");
        if event == WEBKIT_LOAD_STARTED {
            self.drop_contexts(|c| c.view == Some(serial));
        } else if event == WEBKIT_LOAD_FINISHED {
            let waiting = self.views.iter_mut().find(|v| v.serial == serial).and_then(|v| v.waiting.take());
            if let Some(reply) = waiting {
                reply.value("null");
            }
            // A background page without the `ready` call (an older api.js,
            // or a page that never finished loading its scripts) counts as
            // ready once loaded, so nothing waits forever.
            let background = self.views.iter().find(|v| v.serial == serial).filter(|v| v.kind == Kind::Background).map(|v| v.ext.clone());
            if let Some(ext) = background {
                if self.loaded(&ext).is_some_and(|l| !l.ready) {
                    self.background_ready(&ext);
                    self.first_run(&ext);
                }
            }
        }
    }

    /// runtime.onStartup every start, and runtime.onInstalled the first
    /// time this version of the extension runs.
    fn first_run(&mut self, ext: &str) {
        let Some(loaded) = self.loaded(ext) else { return };
        let version = loaded.manifest.version.clone();
        let marker = self.data.join(ext).join("version");
        let previous = std::fs::read_to_string(&marker).ok();
        if previous.as_deref() == Some(version.as_str()) {
            if !self.first_views_before {
                self.fire(ext, "runtime", "onStartup", json!([]), false);
            }
            return;
        }
        let _ = std::fs::create_dir_all(marker.parent().expect("under data"));
        let _ = std::fs::write(&marker, &version);
        let details = match previous {
            None => json!({"reason": "install"}),
            Some(old) => json!({"reason": "update", "previousVersion": old}),
        };
        self.fire(ext, "runtime", "onInstalled", json!([details]), false);
    }

    /// The file name an extension asked for `uri`, taken.
    pub fn download_name(&mut self, uri: &str) -> Option<String> {
        self.download_names.remove(uri)
    }

    pub fn download_finished(&mut self, uri: &str, path: &str) {
        self.fire_all("downloads", "onChanged", json!([{"id": 0, "url": uri, "filename": {"current": path}, "state": {"current": "complete", "previous": "in_progress"}}]));
    }

    /// A command from the window, derisk, or a later `danube` run.
    pub fn command(&mut self, e: &mut Engine, command: ExtCommand) {
        match command {
            ExtCommand::Action(ext) => self.action(e, &ext),
            ExtCommand::ClosePopup => self.close_popup(e),
            ExtCommand::PopupInput(inputs) => self.popup_input(e, inputs),
            ExtCommand::Options(ext) => self.open_options(e, &ext),
            ExtCommand::Enable(ext, enabled) => {
                if let Err(error) = self.store.set_enabled(&ext, enabled) {
                    warn!(error = %error, "cannot change an extension's state");
                }
                self.reload(Some(&ext));
            }
            ExtCommand::Reload(which) => {
                self.first_views_before = true;
                self.reload(which.as_deref());
            }
            ExtCommand::Shortcut { ext, command } => self.shortcut(e, &ext, &command),
            ExtCommand::ViewLoad(serial, event) => self.view_load(serial, event),
            ExtCommand::ViewClosed(serial) => {
                if self.popup == Some(serial) {
                    self.close_popup(e);
                } else {
                    self.close_view(serial);
                }
            }
            ExtCommand::TabLoad(id, event) => self.tab_load(e, id, event),
        }
    }
}

/// executeScript's results, one per frame, answered once all are in.
struct Gather {
    results: Vec<Value>,
    left: usize,
    reply: Option<Reply>,
}

impl Gather {
    fn add(&mut self, result: Value) {
        self.results.push(result);
        self.left = self.left.saturating_sub(1);
        if self.left == 0 {
            if let Some(reply) = self.reply.take() {
                reply.value(&Value::Array(std::mem::take(&mut self.results)).to_string());
            }
        }
    }
}

/// Routes a script message to the runtime; a reply the engine cannot take
/// now is an error the page sees, rather than a promise that never settles.
fn message(origin: Origin, text: String, reply: Reply) {
    let taken = try_with(|e| with(e, |rt, e| rt.handle(e, origin, text, reply)));
    // `reply` moved into the closure; if the engine was busy the closure
    // never ran and the reply dropped, which WebKit reports to the page.
    if taken.is_none() {
        debug!("a script message arrived while the engine was busy");
    }
}

fn popup_frame(buffer: &Buffer) {
    let (width, height) = (buffer.width(), buffer.height());
    let Ok(pixels) = buffer.pixels() else { return };
    if width == 0 || height == 0 {
        return;
    }
    if let Some(image) = to_image(pixels, width, height) {
        emit(Event::PopupFrame(Arc::new(image)));
    }
}

fn origin_of(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.split('/').next().unwrap_or("")),
        None => "null".to_owned(),
    }
}

fn tab_json(entry: &crate::engine::Entry, active: bool) -> Value {
    json!({
        "id": entry.id,
        "index": entry.id.saturating_sub(1),
        "windowId": 1,
        "active": active,
        "highlighted": active,
        "selected": active,
        "pinned": false,
        "url": entry.page.uri(),
        "pendingUrl": Value::Null,
        "title": entry.page.title(),
        "status": if entry.page.is_loading() { "loading" } else { "complete" },
        "incognito": false,
        "audible": false,
        "discarded": false,
        "autoDiscardable": true,
        "groupId": -1,
        "frozen": false,
    })
}

fn window_json(e: &Engine, id: Option<u64>, tabs: Option<Vec<Value>>) -> Value {
    let (width, height, _) = e.size;
    let mut window = json!({
        "id": id.unwrap_or(1),
        "focused": e.focused,
        "type": if id.is_some_and(|id| id >= 1000) { "popup" } else { "normal" },
        "state": "normal",
        "alwaysOnTop": false,
        "incognito": false,
        "left": 0,
        "top": 0,
        "width": width,
        "height": height,
    });
    if let Some(tabs) = tabs {
        window["tabs"] = json!(tabs);
    }
    window
}

fn alarm_json(name: &str, alarm: &Alarm) -> Value {
    let mut v = json!({"name": name, "scheduledTime": alarm.scheduled});
    if let Some(period) = alarm.period {
        v["periodInMinutes"] = json!(period);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_split() {
        assert_eq!(split_uri("chrome-extension://abc/popup/index.html?x=1#y"), Some(("abc", "popup/index.html")));
        assert_eq!(split_uri("chrome-extension://abc"), Some(("abc", "")));
        assert_eq!(split_uri("https://a.example/"), None);
        assert_eq!(origin_of("chrome-extension://abc/x.html"), "chrome-extension://abc");
    }

    #[test]
    fn api_is_configured() {
        assert!(API.contains("__DANUBE_CONFIG__"));
        assert_eq!(content_type("a/b.wasm"), "application/wasm");
    }
}
