# Sonne

Sonne is the OS's code editor, the project's fork of
[Zed](https://zed.dev) ([losos-project/Sonne](https://github.com/losos-project/Sonne)),
and what a text or source file opens in (`nixos/modules/sonne.nix`). It
draws its windows through mcsapi by default (Zed's GPUI scene, painted by
egui), and its agent comes with the skills built into it (`crates/agent_skills/builtin`
in Sonne): writing new skills, customizing the desktop by prompting, and
reducing a person's online footprint. The pm recipe language is built in too, and when
`derisk` is on `PATH`, as it is here, Sonne registers `derisk mcp` as a
context server, so its agent can read and drive the desktop. derisk's
palette opens Sonne's agent on whatever was typed ("Ask Sonne's agent"),
through the `zed:` links Sonne's desktop entry claims.

`sonne` on the command line opens files and folders in a running Sonne or
starts one; `sonne --wait FILE` makes it an `$EDITOR`.

## Where it ships

Sonne is on the PC image, which is the desktop, laptop and tablet image
alike. The Halium image ([Phones](halium.md)) goes without: it is one image
for every Treble phone, and a desktop editor with no touch input does not
fit a screen under 600 logical pixels on its short side.

## What it opens

Text files, and source files by their own types (C, C++, Rust, Python, Go,
Java, Markdown, JSON, TOML, YAML, XML, shell scripts and a few more), open in
Sonne from Files, the palette and `xdg-open`, all of which read
`mimeapps.list`. The types are listed one by one because `xdg-open` outside
GNOME and KDE looks a type up as it is, not through `text/plain`, which
they are all subclasses of. derisk's own Text Editor is built into derisk
and stays; it never claimed any of them.

## How it is built

`nixos/pkgs/sonne.nix` builds Sonne the way nixpkgs builds `zed-editor`,
from a pinned revision (`fetchgit`, [Trust](trust.md)). The differences:

- **Thin LTO.** Upstream's release profile is fat LTO in one codegen unit:
  one link that runs for most of an hour on one core and needs more memory
  than CI's runner has. The package builds thin LTO over 16 units instead.
- **libwebrtc from nixpkgs' own package set.** Sonne took out Zed's
  collaboration crates, but its audio crate still runs microphone input
  through WebRTC's audio processing. From the overlay's package set,
  libwebrtc's FFmpeg and SDL would reach the [patched GTK](gtk-qt-phone.md)
  through libdecor, and CI would compile WebRTC and FFmpeg, so it comes from
  plain nixpkgs and substitutes from cache.nixos.org.
- **Stripped in full.** Every byte is in `/usr`, whose update slot is a
  fixed 8G on an installed disk ([Where the OS lives](layout.md)); a full
  strip takes the editor from 473M to 333M, and the RUNPATH is pointed at
  libwebrtc's library rather than its 268M of headers. Sonne still adds about half a
  gigabyte to the store, which only fits because `/usr` is compressed.
- **No remote server and no tests.** The remote server is what Zed copies to
  a host it opens over SSH; Sonne's tests are its own repository's.
- **Updates come with the system.** The auto-updater is off and says so.

Moving the pin is a new `rev` and `hash` in `sonne.nix`, and a new
`cargoHash` when Sonne's `Cargo.lock` changed.
