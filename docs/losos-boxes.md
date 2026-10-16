# LosOS boxes

A [LosOS](https://losos.dasmat.us) box is the project's home server: a
mini-PC running Nextcloud and Forgejo, with no SSH and no shell, managed
from its admin pages in a browser. LosOS Desktop finds the boxes on the
local network by themselves and offers each one in the palette and the app
list, and carries `losos`, the box's own terminal client, so a box can be
managed from the terminal too (`nixos/modules/losos-boxes.nix`).

## Finding boxes

Type a box's name, or "LosOS", into the palette. Each box on the network is
an app named after the box, `mattbox`, described as "LosOS v0.4.0 box at
mattbox.local". Opening it opens the box's admin pages,
`http://mattbox.local/`, in the default browser. Its one action, **Manage
in the terminal**, opens the box in `losos` in a foot window, signing in
first when `losos hosts` does not list it yet (below); if anything fails,
the window stays open until Enter so what `losos` said can be read.

`losos-boxes watch`, a user service started with the session, keeps those
entries. It browses two DNS-SD types through systemd-resolved's Varlink
interface (`io.systemd.Resolve.BrowseServices`), since resolved already
answers mDNS on this OS ([systemd](systemd.md)) and Avahi would be a
second responder on port 5353:

- `_losos._tcp`, which a box announces on its front port, with its release
  in the TXT record (`version=v0.4.0`).
- `_workstation._tcp`, which every box announces, as do most Linux
  machines running Avahi. Boxes installed before the `_losos._tcp`
  announcement announce only this until they update, so each workstation
  is asked for `/setup/state.json` on port 80, which only a box serves. A
  box found both ways is one entry.

It resolves each instance with `ResolveService` and writes
`losos-box-<host>.desktop` into `~/.local/share/applications`, which
derisk reads each time the palette opens. A box that sends mDNS's goodbye,
or whose records expire, loses its entry. The entries are removed when the
service stops, at sign-out, and again when it starts, so a box that left
while nobody watched is not offered. Only a host name gets into an entry:
anything on the network can answer mDNS, and the name ends up on an `Exec`
line. `losos-boxes list` prints what it finds within a few seconds, one
box per line, with its address and release.

## The terminal client

`losos` shows the same panes as a box's admin pages, in the same words and
colours. Every request it sends is signed by your GPG key and encrypted to
the box, and the box answers the same way, so it needs no SSH and plain
`http://` is safe for it. The handbook's
[terminal client](https://losos.dasmat.us/manual/terminal/) page is the
whole guide; on LosOS Desktop it comes down to:

```sh
losos key import --gpg you@example.com
losos login http://mattbox.local
losos
```

A box accepts only keys that were on the stick it was installed from, so
the key has to be one of those, and the token `login` asks for comes from
**Settings → Access tokens** in the box's admin pages. `login` shows the
box's key in groups of four to compare with that pane before you accept
it.

`losos` keeps keys and tokens in the Secret Service, which GNOME Keyring
answers on this desktop, through libsecret's `secret-tool`, and never
writes them to a file. `key import --gpg` reads the key from gpg. Both
tools are on its `PATH`, after the user's own, so a gpg installed with
[pm](pm.md) is the one it uses.

## How it is built

`nixos/pkgs/losos.nix` builds the `losos` crate from the LosOS repository
(`github.com/dasmatus/losos`, `admin-ui/tui`) at a pinned commit, the same
crate and `cargoHash` as that repository's own `.#losos`. Its build script
reads the admin UI's catalogue, palette, pane keywords and the salmon, and
the crate links the envelope crate in `backend/access`, so a sparse
checkout fetches those directories and nothing else, about 3 MB of a tree
of 140. That is why it is a `fetchgit` pin and not a `components/`
submodule ([trust](trust.md)): the flake copies its submodules on every
evaluation.

Moving it to a newer LosOS is the `rev` and `hash` in that file, the
`cargoHash` when `admin-ui/tui/Cargo.lock` changed, and the version, which
is the tag in the LosOS tree's `nix/version.nix` at that commit. The
client's own test suites, a mock box and edge over TLS and a pseudo
terminal, run in the LosOS repository's CI, not here.

`losos-boxes` is `src/losos-boxes`, built by `nixos/pkgs/losos-boxes.nix`
with its unit tests. `nixos/tests/losos-boxes.nix` boots two boxes as far
as mDNS and nginx go, one announcing `_losos._tcp` and one from before it,
and a desktop with resolved and no Avahi, which lists both, writes their
entries and drops the first one's when it leaves the network.
