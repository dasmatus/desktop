# LosOS boxes

A [LosOS](https://losos.dasmat.us) box is the project's home server: a
mini-PC running Nextcloud and Forgejo, with no SSH and no shell, managed
from its admin pages in a browser or from `losos`, its terminal client.
LosOS Desktop finds the boxes on the local network by themselves and
offers each one on the desktop.

On a PC all of that happens in one [machine](hypervisor.md), the **LosOS
boxes machine**, and not on the host: it finds the boxes, holds `losos`
with its keys and tokens, opens the admin pages, and is the way the other
machines reach a box. The host only lends it its resolver. A phone has no
machines, so there the session does it itself (below).

## The LosOS boxes machine

Make it in the Machines app from the **losos-boxes** template, or with

```sh
pm machine create boxes --image /run/current-system/sw/share/losos/machines/losos-boxes.qcow2 --label green
pm machine start boxes
```

Its template is `nixos/guest/boxes.nix`: the LosOS guest's machine
(`nixos/guest/common.nix`, [the guest template](hypervisor.md#the-guest-template))
with `losos`, `losos-boxes`, Danube, GNOME Keyring and the GitHub CLI,
`gh`, which `losos ci setup` uses to give a repository's Actions a key for
a box, and without Flatpak.
Each box it finds is an app of that machine on the desktop, named after
the box, `mattbox`, and described as "LosOS v0.4.0 box at mattbox.local";
the desktop shows it as "mattbox in boxes", framed in the machine's
colour. Opening it opens the box in `losos` in a derisk-terminal window in
the machine, signing in first when `losos hosts` does not list it yet
(below); if anything fails, the window stays open until Enter so what
`losos` said can be read. Its one action, **Open the admin pages**, opens
`http://mattbox.local/` in Danube, in the machine too.

## Finding boxes

`losos-boxes watch --terminal`, a service of the machine's user, keeps
those entries. It browses two DNS-SD types through systemd-resolved's
Varlink interface (`io.systemd.Resolve.BrowseServices`):

- `_losos._tcp`, which a box announces on its front port, with its release
  in the TXT record (`version=v0.4.0`).
- `_workstation._tcp`, which every box announces, as do most Linux
  machines running Avahi. Boxes installed before the `_losos._tcp`
  announcement announce only this until they update, so each workstation
  is asked for `/setup/state.json` on port 80, which only a box serves. A
  box found both ways is one entry.

It resolves each instance with `ResolveService` and writes
`losos-box-<host>.desktop` into `~/.local/share/applications`, where the
machine's `losos-guest-run --list` finds it for the desktop. A box that
sends mDNS's goodbye, or whose records expire, loses its entry. The
entries are removed when the service stops and again when it starts, so a
box that left while nothing watched is not offered. Only a host name gets
into an entry: anything on the network can answer mDNS, and the name ends
up on an `Exec` line. `losos-boxes list` prints what it finds within a few
seconds, one box per line, with its address and release.

The resolved it browses through is the host's. The machine does no mDNS
of its own ([names on the network](hypervisor.md#names-on-the-network)),
and its multicast would not get past the host's NAT to the LAN anyway,
while the host's resolved does mDNS on every network the host is on. So
`LOSOS_BOXES_RESOLVE` points the machine's losos-boxes at
`/run/losos-boxes/resolve`, a socket that `systemd-socket-proxyd` carries
to the host over vsock, port 7351. There `losos-boxes relay`
(`nixos/modules/losos-boxes.nix`), a system service under an account of
its own, takes the connection and finds the QEMU whose vsock device has
the address the kernel stamped on it, by its command line in `/proc`. The
pid file on that command line names the machine's folder, and the relay
passes the connection to resolved's Varlink socket only if pm's record
there, `machine.json`, belongs to the account QEMU runs as and says the
machine was made from `losos-boxes`; any other machine is refused. A
machine is its owner's, in their home, so the relay may read any file
(`CAP_DAC_READ_SEARCH`) and write only its own. Varlink's resolve
interface only looks things up, so the machine can browse and resolve
through the host and change nothing there.

## From the other machines

The other machines reach a box through the LosOS boxes machine, so it is
the one machine that talks to boxes:

1. `losos-boxes route`, a system service of the LosOS boxes machine,
   finds the boxes the same way and sends their names to the host's relay
   on vsock port 7352 whenever they change.
2. The relay writes them, at the address the machines' DHCP gave that
   machine (dnsmasq's lease for the MAC address on its QEMU's command
   line), into `/run/losos-boxes/hosts/<uid>-<machine>`, named after its
   owner's account as well as the machine, since two people may each have
   a machine called `boxes`. The machines' dnsmasq, `losos-machines-dns`,
   the DNS server of every machine, reads that directory whenever it
   changes (`--hostsdir`, from `losos.machines.network.hostsDirs`), so
   `mattbox.local` is answered with the LosOS boxes machine's address
   before dnsmasq would ask the host's resolved. `losos-boxes-dns`, which
   added the directory to libvirt's network once at boot and restarted it,
   went with libvirt: the machines' dnsmasq is the OS's own and takes it
   as an argument.
3. `route` takes each connection on ports 80 and 443, reads the name it is
   for from its `Host` header or its TLS ClientHello's server name, and
   passes it on to that box as it is, TLS included: the certificate the
   other machine sees, and `losos` pins, is the box's own. A name that is
   not a box present gets nothing.

When the LosOS boxes machine stops, its hosts file goes, and box names are
answered as any other `.local` name again: by the host, with the box's
own address. Every person's machines are on the one network and ask the
one dnsmasq, so one person's LosOS boxes machine names the boxes for
everyone's machines while it runs. Only ports 80 and 443 go through it,
which is where a box serves its admin pages, its cloud and its Git; an
app's own port on a box is reached directly. And nothing stops a machine from dialling a box's
address it knows: what goes through the LosOS boxes machine is what is
reached by name.

## On a phone

The GSI has no machines, so the session runs `losos-boxes watch` itself
and carries `losos`: each box is an app of the phone, opening it opens the
admin pages in the default browser, and its action, **Manage in the
terminal**, opens `losos`.

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
answers in the LosOS boxes machine (and in a phone's session), through
libsecret's `secret-tool`, and never writes them to a file. Nobody signs
in to the machine, so its keyring is not unlocked by a password at
sign-in: the first time `losos` stores something, the keyring asks for a
password for itself in a window of the machine. `key import --gpg` reads the key from gpg. Both
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
and a phone's session with resolved and no Avahi, which lists both, writes
their entries and drops the first one's when it leaves the network.
`nixos/tests/machines.nix` makes a LosOS boxes machine on a host whose LAN
has a box, checks that it offers the box, that another machine's request
for the box's name goes through it and gets the box's answer, that a name
which is not a box is refused, and that stopping it removes its names.
