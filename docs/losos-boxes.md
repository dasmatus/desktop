# LosOS boxes

A [LosOS](https://losos.dasmat.us) box is the project's home server: a
mini-PC running Nextcloud and Forgejo, with no SSH and no shell, managed
from its admin pages in a browser. LosOS Desktop carries `losos`, the
box's own terminal client, so a box can be managed from the terminal too
(`nixos/modules/losos-boxes.nix`).

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
