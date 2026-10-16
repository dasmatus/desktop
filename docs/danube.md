# Danube

Danube is the OS's web browser and the default for links and web pages
(`nixos/modules/danube.nix`). It is [WPE WebKit](https://wpewebkit.org),
the WebKit port with no toolkit of its own, drawn inside a window built
from mcsapi's components in derisk's theme (`src/danube`). It is the web
view and nothing more: derisk's command palette is its address bar, its
tab switcher and its list of extensions, and derisk's top bar carries its
menus. It runs Chrome's extensions, unmodified, with a runtime of its own. It also opens
the sign-in page of a network that wants one first (a captive portal), and
it loads the content blockers that [system-wide ad blocking](adblock.md)
compiles.

## How it is put together

WPE's headless platform renders each page into a buffer and hands it over
instead of putting it on screen. Danube copies the active tab's frame into
an egui texture and draws it, so what little interface the window has (a
progress line, a permission request's bar, a crashed page's notice,
toasts) is ordinary mcsapi code: the same components as derisk's apps,
recoloured whenever derisk publishes a new theme
(`$XDG_RUNTIME_DIR/derisk/theme.json`). Pointer, wheel, touch and key input
over the page goes back to WebKit as WPE events; typed text goes as one key
press per character, and Ctrl+C, Ctrl+X and Ctrl+V cross between WebKit's
clipboard and the desktop's.

WebKit runs on the process's main thread under GLib's main loop, the window
on a second thread under winit's (`engine.rs` and `ui.rs`). Two bounded
channels (crossbeam's) join them and nothing is locked: the window sends
commands, WebKit sends events (a tab opened, its title, a frame, a
permission asked) and asks for a repaint, and the window keeps its own
picture of the tabs from them. A frame that does not fit the channel is
dropped for the next. The threads beside them, derisk's menus, later
`danube` runs handing over URLs, the captive portal's recheck, are scoped
to the same `run` and end with WebKit.

There are no Rust bindings for WPE's 2.0 API, so the crate makes its own:
`build.rs` runs bindgen over the installed `wpe/webkit.h` and
`wpe/headless/wpe-headless.h` at build time, allowlisted to WebKit's and
WPE's symbols and the handful of GLib calls used, and `ffi.rs` includes the
result. Every signature and enum value is the library's own, so a WebKit
update that changes one fails to compile rather than at runtime. `webkit.rs`
wraps the objects in types that own one reference each (`Display`,
`WebView`, `View`, `Toplevel`, `Settings`, `NetworkSession`, `FilterStore`
and so on), give it back when dropped, offer the calls as methods, and take
closures for signals, which GObject frees with the connection. `engine.rs`
holds only the tab logic on top of them and has no `unsafe` in it.

nixpkgs has only WebKitGTK, so `nixos/pkgs/wpewebkit.nix` builds the WPE
port from the WPE release of the same WebKit version, with nixpkgs'
WebKitGTK patches. It builds only the WPEPlatform API (headless and Wayland
platforms) and nothing upstream substitutes it, so CI compiles it once per
architecture and keeps it in the project's cache.

![Danube's main thread runs WebKit under GLib's loop and its window thread runs winit and egui; two bounded channels carry commands one way and events and frames the other; each web process has its own sandbox; tabs and menus go to derisk over the agent socket and picked commands come back](images/danube.svg)

## The window

The window is the page, and nothing else: no toolbar, no address bar, no
tab strip. Danube is the web view; addresses, tabs and extensions are
derisk's command palette's to manage, and derisk's top bar
shows its menus. The window's title names the tab in front. Over the page
the window draws only what has to be there: a progress line along the top
while a page loads, an extension's popup in the top right corner, a bar for a site's permission request, a notice on a
page whose web process died, and the link under the pointer. A phone
window (short side under 600 logical pixels, as in
[GTK and Qt on a phone](gtk-qt-phone.md)) sends a mobile User-Agent
string ("Android", "Mobile"), so sites serve their phone version from the
next page loaded.

The palette is the address bar. Ctrl+L (or Alt+D) in Danube asks derisk
to show it, and what is typed there that reads as a web address
(`example.com`, `localhost:8080`, a full URL) opens in Danube, in a new
tab. Anything else the palette searches for with the desktop's search
engine, chosen by the choice screen and Settings, Default apps
([choice screens](choice-screens.md)). Danube has no search setting of
its own.

![derisk's command palette with "losos desktop" typed: matching apps first, then "Search the web for losos desktop" with the chosen engine](images/choice-screens-palette-search.png)

The palette's Tabs and Extensions headings are derisk's bundled browser
palette plugin (a WebAssembly plugin of the palette, like its other
sources) showing what Danube registers: `register_palette` on the agent
socket, with the tabs (id, title, URL, which is active) and the
extensions (id, name, whether enabled, whether it has options), sent again
whenever they change. What is picked there comes back as a `palette`
event with a command: `activate_tab`, `close_tab`, `new_tab`, or `open`
with the http(s) URL derisk made of the typed text (`https://` for a
bare host, `http://` for localhost), which Danube opens in a new tab; `enable_extension`,
`disable_extension` and `extension_options` do what they say. Without the plugin (an
older derisk) the registration is refused and the menus below still list
the tabs.

The menus are derisk global menus, in the top bar and in the command
palette, where "Tabs › Wikipedia" switches to that tab and "Close Tab ›
Wikipedia" closes it:

- **Page**: the address, "Secure" or "Not secure", and how many of
  [system-wide ad blocking](adblock.md)'s filter sets are in force, as
  lines the top bar shows and the palette leaves out; then Open Address
  (the palette) and Copy Address.
- **Tabs**: each by its title, New Tab, Next Tab, Previous Tab.
- **Navigate**: Back, Forward, Reload, Stop.
- **Close Tab**: each by its title.
- **Extensions**: each extension's button (its popup, or what it does
  when clicked), its options page, and Enable or Disable.

Danube registers them over derisk's agent socket
(`$XDG_RUNTIME_DIR/derisk/agent.sock`), finding its window by app id
(`org.losos.Danube`) and title, and sends them again whenever a tab
opens, closes or changes its title, address or security. Outside a derisk
session there is no palette and no menus, and only the keyboard reaches
the tabs.

Keys: Ctrl+L (or Alt+D) the palette, Ctrl+T a new tab (and the palette
for its address), Ctrl+W close, Ctrl+Tab and Ctrl+Shift+Tab the next and
previous tab, Ctrl+R or F5 reload, Alt+Left and Alt+Right back and
forward; a middle click on a link opens it in a tab behind. An
extension's own shortcuts (its manifest's `commands`, Bitwarden's
Ctrl+Shift+L to fill a login, say) work too, and Esc closes its popup.

Downloads go to `~/Downloads`, never overwriting a file there; see
[Downloads](#downloads) for how they are fetched and shown.

## Downloads

WebKit decides what is a download: a response it cannot show, a link with
`download`, an extension's `downloads.download`. Once its response has
arrived, Danube names the file (the server's name, or the extension's)
and, for a plain GET over HTTP or HTTPS, stops WebKit's transfer and hands
the file to [aria2](https://aria2.github.io/)
(`src/danube/crates/danube/src/downloads.rs`). aria2 asks for the final
address after WebKit's redirects with what WebKit would have sent: the
cookies WebKit's jar holds for it (HttpOnly ones included), the request's
Referer, and the window's User-Agent. It splits the file over one
connection per hardware thread (`split`), at most 16 to one server, which
is as many as aria2 allows (`max-connection-per-server`), in ranges of at
least 1 MiB. A server that caps each connection's speed then delivers the
file that many times faster; one that does not answer range requests
gets one connection, as before.

One aria2c runs per browser, started with the first download, inside
Danube's sandbox, so it reaches the network and `~/Downloads` and nothing
else of the home. Danube talks to it over JSON-RPC on a loopback port,
with a random secret that aria2 reads from a file only the user can read,
in the sandbox's private `/tmp`, removed once aria2 answers, and never sees
on its command line. aria2 stops with Danube (`--stop-with-process`), and
a download Danube had not finished is removed with it.

WebKit keeps the downloads aria2 should not take: a POST's answer, a
`blob:` or `data:` URL, an extension's own pages. It also takes over any
download aria2 failed on before the first byte, such as one behind a login
only the page's session knows, asking again from the same tab so its
cookies go with it, and every download from then on if aria2 could not be
started at all.

Each download is a desktop notification while it runs: the file's name,
how much of how much has arrived, the speed and aria2's connections, a
progress bar, and a Cancel button. It stays until the download ends (no
timeout, `resident`), is replaced in place at most once a second, and is
replaced a last time by one saying it finished, failed or was cancelled,
which the server then lets expire. A notification closed while its
download runs is not shown again until it ends. derisk serves the
notifications; with no notification server on the bus, Danube says how
each download ended in a toast instead.

## Extensions

Danube runs Chrome's extensions, Manifest V2 and V3, as they come from
the Chrome Web Store, and Firefox's written to the same API. WPE WebKit
has no extension runtime of its own (WebKitGTK and WPE leave that to the
browser), so Danube has one (`src/danube/crates/danube/src/extensions`).
Bitwarden's Chrome extension, unmodified, signs in to a self-hosted
server, unlocks, and fills a login from its popup, from Ctrl+Shift+L and
from its menu in the page's field.

```sh
danube install-extension bitwarden.crx   # a .zip, .crx, .xpi or a directory
danube list-extensions
danube uninstall-extension nngceckbapebfimnlniiiahkandclblb
```

These run outside the sandbox, since the file can be anywhere, and tell
a running Danube to load the change. An extension is unpacked into
`~/.local/share/danube/extensions/<id>/`, the id being the one Chrome
gives it (from the key in its manifest or its `.crx` header, so it keeps
the id its server knows), and keeps its storage in
`~/.local/share/danube/extension-data/<id>/` as JSON files.
`~/.config/danube/extensions` says which are turned off. The Extensions
menu and the palette's Extensions heading turn each on or off and open
its options page.

How it runs:

- **Its files** come from a `chrome-extension://<id>/` scheme of Danube's,
  each page with the manifest's content security policy. A web page may
  load only what the manifest lists in `web_accessible_resources`; WebKit
  lets a page's frame reach that much because Danube puts those files on
  each tab's CORS allow list.
- **Its background** (a page, or a service worker run as one, since WebKit
  has no worker to give an extension) is a hidden web view, as are its
  popup and an offscreen document. The popup's frames go to the window,
  which draws them over the page and sends it the pointer and keys.
- **Its content scripts** are WebKit user scripts in an isolated world of
  the extension's own, with its CSS as user style sheets. Each tab has a
  content manager of its own, so the tab a message comes from is known
  without the page saying.
- **`chrome.*`** (and `browser.*`) is `api.js`, run first in every one of
  those contexts. Each call is a script message to the runtime on
  WebKit's thread, which answers it: messaging and ports, tabs, storage,
  scripting, alarms, the action's popup and badge, commands, i18n,
  downloads, notifications (as Danube's own toasts), offscreen documents.
  What a context has to be told (a message, an event) it collects by
  asking with a message the runtime answers only once there is
  something, since WebKit's own calls into a page reach only its main
  frame.
- **A sandboxed page in a web page's frame** (Bitwarden's menu in a login
  field) has an opaque origin, which WebKit lets load nothing from the
  extension's scheme. Danube serves such a page with its scripts and its
  style sheets written into it, the scripts' hashes added to its policy,
  and swaps a style sheet it switches to (a theme) for the same file
  written in.

What an extension cannot do here:

- See or change requests: `webRequest` takes listeners and never calls
  them, and `declarativeNetRequest` keeps no rules. Blocking is
  [system-wide ad blocking](adblock.md)'s job.
- Talk to a native program (`runtime.connectNative`), so Bitwarden cannot
  reach its desktop app for biometric unlock.
- Run in a page's own world (`world: "MAIN"`) anywhere but its main frame.
- Reach another of its views: `extension.getViews()` is empty and
  `getBackgroundPage()` null, since each view is a web process of its own.
- Use `cookies`, `contextMenus`, `history`, `bookmarks`, the side panel or
  `identity`, which answer as if empty or fail.
- Fetch one of its own files from a content script unless the manifest
  lists it as web-accessible, as a page could not either.

## Sites' permissions

A page's request for the camera, microphone, screen, location,
notifications, clipboard, pointer lock, the list of devices, protected
(DRM) media or its cookies on other sites is denied unless the person
allows it. Danube asks in a bar over the page, for that site, once or
remembered; remembered answers are lines in `~/.config/danube/permissions`
(`host kind allow|deny`), which can be edited or deleted.

## Captive portals

`danube captive-watch`, a user service in every session, rechecks the
network whenever systemd-networkd's state changes: it fetches GNOME's
check, `http://nmcheck.gnome.org/check_network_status.txt`, over plain HTTP,
which a portal intercepts, sending the request line, the host and "Danube"
as the User-Agent and nothing else. Any answer but a 200 saying
"NetworkManager is online" means a portal, and it opens `danube --captive-portal`: one window titled
"Sign in to network", no tabs, a profile that ends with the window, and no
content blockers, since a portal's page is often built from what they
block. That window checks again every three seconds and closes itself once
the network lets traffic through. The desktop entry's "Sign in to network"
action opens it by hand.

## The desktop background

derisk can show a web page as the desktop background (Settings,
Wallpaper, "Web page"), and Danube draws it. derisk starts
`danube --background URL --size WIDTHxHEIGHT --scale S` at the screen's
size, as it starts ffmpeg for a video wallpaper, and reads frames from
its stdout: a width and a height (32-bit little-endian), then that many
premultiplied RGBA pixels, for every frame the page renders
(`src/danube/crates/danube/src/background.rs`). There is no window. An
`http://`, `https://` or `file:///` address is all it takes; a local page
can load the pictures and scripts beside it, since the sandbox shows its
folder read-only.

The page gets nothing of the person's: a profile that ends with it, no
extensions, every permission refused, no sound, no keyboard or pointer.
It gets the content blockers like any tab. When a window covers the
screen (or in low power mode, as the Settings app chooses), derisk writes
`hide` on Danube's stdin: WebKit stops painting the page and slows its
timers, and Danube pauses its CSS animations, videos and sounds, which
WebKit's compositor would otherwise keep running; `show` resumes exactly
those. A frame derisk has not read yet is replaced by the next instead of
queueing. A load that fails, or a web process that dies, ends the run
with the reason on stderr, and derisk shows its own background instead.
Danube's stdout carries only frames: the web processes it starts write
theirs to stderr.

## Sandbox and hardening

Two sandboxes, both built with the [Hakoniwa](https://github.com/souk4711/hakoniwa)
crate (`src/danube/crates/danube-sandbox`).

- **The browser.** Started outside it, `danube` builds a container and runs
  itself again inside: its own user, mount, PID, IPC and UTS namespaces;
  the Nix store, `/etc`, the GPU driver and sysfs read-only; `/dev/dri`;
  an empty home with only `~/.local/share/danube`, `~/.cache/danube`,
  `~/.config/danube` and `~/Downloads` writable and the user's fonts,
  icons and derisk's settings read-only; an empty runtime directory with
  only the Wayland, PipeWire and PulseAudio sockets and derisk's directory;
  and the session bus through xdg-dbus-proxy, which lets through only the
  desktop portals, notifications and the accessibility bus. Landlock
  restricts the same paths again, and a seccomp filter denies the system
  calls Flatpak denies (keyctl, ptrace's relatives, kexec, bpf and the
  rest), except the namespace calls the next sandbox needs. The network
  is the host's, since WebKit's network process needs it.
- **Each web process.** WebKit puts every web process in a sandbox of its
  own, built with bwrap upstream. One patch
  (`nixos/pkgs/patches/wpewebkit`) lets the embedder name a launcher that
  takes bwrap's arguments; Danube names `danube-sandbox`, which reads the
  arguments WebKit gives and builds the same sandbox with Hakoniwa:
  WebKit's own filesystem layout and its seccomp filter, loaded last, with
  the descriptors WebKit hands over carried in. An argument it does not
  know is an error, so a WebKit update that asks for more cannot run a web
  process with less.

WebKit is built with level-3 fortify checks, stack clash protection,
automatic variables zeroed, eager binding (the whole GOT read-only), and
control-flow protection where the CPU has it (CET on x86_64, BTI and PAC on
aarch64). WebDriver, the remote inspector and WebKit's unshipped
(experimental) features, WebXR among them, are not built. JavaScriptCore's
JIT compilers are off by default (`JSC_useJIT=false`): scripts and
WebAssembly run in its interpreters, slower, and a page can no longer have
the browser write and run machine code, which most WebKit exploits rely on.
`javascript.jit = true` in the user's settings turns them back on. WebKit keeps its own allocator (libpas, with its Gigacage)
for JavaScript and DOM objects; everything else it allocates through
`malloc`, which is hardened_malloc on a PC ([the allocator](allocator.md)).
Tracking prevention (WebKit's ITP) is on, and there is no keyring for
WebKit to save passwords in.

The sandboxes need unprivileged user namespaces, which NixOS's kernel
allows. A phone's own kernel may not: see [what is not done](not-done.md).

## Settings

The user's `~/.config/danube/settings.conf`, `key = value` per line; a
key left out keeps its default. There are no NixOS options for them:
`losos.danube.home` and `losos.danube.jit`, which wrote
`/etc/danube/settings.conf` under it, went because nothing set them.

| Key | Meaning |
| --- | --- |
| `home` | What a new tab opens (an empty page by default) |
| `javascript.jit` | `true` turns JavaScriptCore's JIT compilers on |
| `downloads.aria2` | `false` leaves downloads to WebKit's own downloader, one connection each (on by default) |
| `downloads.connections` | How many connections aria2 splits a download over (one per hardware thread by default; at most 16 go to one server) |

There is no `search` key, and `losos.danube.search` is gone with the
address bar: the palette searches with the desktop's engine.

## What it replaced

Danube replaced Uranium, which was ungoogled-chromium under the OS's name:
a launcher that turned it into Chrome for Android on a phone, five patches
and a rebranding script for a build CI could only compile in rounds
(`uranium.yml`), a Flatpak of it served from the proxy, and `uranium-tabs`, a
native messaging host that mirrored its tabs into derisk's menus. Chromium
draws its own interface with its own toolkit, so it could never be an
mcsapi window or follow derisk's theme, and it is not coming back. Profiles
do not carry over: Danube starts with a new one in `~/.local/share/danube`,
and a user's own `~/.config/mimeapps.list` that names `uranium.desktop`
has to name `org.losos.Danube.desktop` instead.
