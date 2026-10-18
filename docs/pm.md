# pm on LosOS

## Administering the machine

There is no sudo, su or run0. An administrator changes the system with
`pm system`, and pm does the work as a user of its own, `pm`:

| Command | What it does |
| --- | --- |
| `pm system start\|stop\|restart\|reload <unit>` | the same to a system unit; a bare name is a service |
| `pm system update [--reboot]` | installs the newest version for the next boot (`systemd-sysupdate.service`), and reboots into it if asked and there was one |
| `pm system run <command>` | runs one command as root |
| `pm system run` | root's shell |

`pm` asks `pmd --system`, a service the system bus starts on the first call
(`pm-system.service`, `nixos/modules/pm.nix`), and it asks polkit whether the
caller may. polkit asks an administrator, a member of `wheel`, to
authenticate as themselves: in derisk's dialog ([polkit and pm](sign-in.md#polkit-and-pm)),
or on the terminal through `pkttyagent` when the session has no agent. A
password typed for `start`, `stop`, `restart`, `reload` or `update` is good
for a few minutes; one for `run` is asked every time. Only then does the
daemon act, as `pm`, through systemd. A command run as root runs in a unit of
its own, `pm-run-<pid>-<n>.service`, on a pseudo-terminal the daemon checks
belongs to the caller, with the caller's terminal relayed to it; `pm` leaves
with its exit status, and the journal records who ran what.

The `pm` user owns nothing and holds no capability. What it may do comes from
one polkit rule, which lets it manage units without asking. A second rule,
evaluated last, refuses unit management to everyone else: without it an
administrator could still `systemctl restart` a service, or use the run0
that systemd itself carries, and become root past pm. Root is never asked, so
the system's own services and the test driver are unaffected, and narrower
rules before it still answer first, such as the one that lets an
administrator ask for a factory reset from Settings (`nixos/modules/disk.nix`).
Everything else polkit governs is as it was: accounts through homed, the
network through NetworkManager, updates from Settings through `sysupdated`,
and Flatpak.

The image's boot test runs `pm system restart` and `pm system run` as root,
checks the daemon runs as `pm`, and asks polkit what it says to `pm` and to
an administrator about managing units directly and about `pm system run`
(`nixos/tests/boot.nix`).

## pm's plugins

`nixos/pkgs/pm-plugins.nix` builds seven components, this repository's three
and pm's own `quickemu`, `sysext`, `sysupdate` and `systemd`, and the image
carries them in
`/run/current-system/sw/share/pm/plugins`. pm loads plugins only from a user's
own `~/.config/pm/plugins`, and only signed by a key that user trusts, so the
image offers them and trusts them for no one:

```
install -Dm644 -t ~/.config/pm/plugins /run/current-system/sw/share/pm/plugins/*.wasm
for p in ~/.config/pm/plugins/*.wasm; do pm sign "$p"; done
```

### With Home Manager

Where Home Manager runs (a build host, or any machine with Nix; the image
itself has no Nix and its users are systemd-homed records), the flake's
`homeModules.pm` installs pm and its plugins instead of that copy:

```nix
{
  imports = [ losos-desktop.homeModules.pm ];
  programs.pm.enable = true;
  # All seven by default; or name the ones you want:
  # programs.pm.plugins = [ "losos-nix" "systemd" ];
}
```

Each plugin is linked into `~/.config/pm/plugins` from the Home Manager
generation, so a plugin taken off the list is uninstalled at the next switch
and a rollback restores the previous set. Activation then runs the same
`pm sign` over each one, with your own key (made and trusted on first use,
as by hand), so trust is still given by you and not by the module.
`extraPlugins` installs other components by name, `signatures` takes a
publisher's `.sig` for one instead of signing it locally, and `trustedKeys`
adds that publisher's key to pm's trust store, which governs build files too.

`nix flake check` builds the module into a Home Manager generation and runs
its signing step against it, then checks that `pm plugins` loads every
plugin with no flags (`nixos/tests/home-manager-pm.nix`). No `home-manager
switch` on a real account has run it.

## Driving the flake from pm

pm stays the system manager here too, so a pm build file can build and check
the flake: `nix build`, `nix flake check`, `nix eval` and the other commands
that read or build are named by the `losos-nix` plugin (`plugins/README.md`),
which pm's fingerprint table otherwise refuses (C2). Two things about pm's
jail shape such a step. `/nix/store` is mounted read-only and the daemon
socket not at all, so the step builds into a store of its own, `nix --store
/build/nix ...`. And a step that says `--offline` gets no network, so the jail
enforces what the step claims; without it the step, and so the whole build
file (C8), is on the host network, because evaluating fetches the inputs and
building substitutes from the project's cache. `nixos-rebuild`, `nix profile`,
`nix run` and `nix flake update` are recognised and deliberately left
unclassified: this OS is updated by systemd-sysupdate, never by switching a
generation, and a pin moves by hand.

## nixpkgs in a pm build

All of nixpkgs is reachable from a pm build file, at the revision
`flake.lock` pins and built for the same stock glibc platform as this OS.
The flake's `legacyPackages` is the image's own package set, so
`.#ripgrep` is nixpkgs' ripgrep, and the same store path the image would carry.
`.#pm-payloads.ripgrep` is what a pm package can actually hold
(`nixos/pkgs/pm-payloads.nix`): the same package from `pkgsStatic`, every output
merged into `usr/bin`, `usr/sbin`, `usr/libexec` and `usr/share`.

Static, because pm extracts a package at `/pkg` and runs it in a jail whose
`/nix/store` is the host's or nothing: a dynamic program names its loader and
libraries by store path. The payload sets `allowedReferences = [ ]`, so Nix
itself refuses one in which any file still names a store path, and says which.
`gnugrep` is refused that way, because `egrep` is a script whose shebang is
bash's store path. That is the check working: such a package could not run
from `/pkg`.

A build file that installs one:

```yaml
name: ripgrep
version: ['15', '1', '0']
dependencies: []
steps:
- stage: Build
  dl_urls: null
  name: build
  run:
  - nix --store /build/nix build --out-link /build/payload github:dasmatus/losos-desktop/<commit>#pm-payloads.ripgrep
- stage: Install
  dl_urls: null
  name: stage
  run:
  - find /build/nix/nix/store -maxdepth 1 -type d -name *-pm-payload-ripgrep-* -exec cp -rT {}/usr /dest/usr ;
```

The second step is `find` rather than `cp /build/payload/usr`, because the
out-link points at `/nix/store/...` and in pm's jail that is the host's store,
not the one at `/build/nix` the step built into. There is no shell, so the
pattern reaches `find` unexpanded.

`losos-nix` holds the flake reference to the same rule as a pin
(`plugins/README.md`): a commit, a path, or a URL with `narHash=` is classified;
`nixpkgs#ripgrep`, `github:NixOS/nixpkgs/nixos-26.05`, `<nixpkgs>` and anything
else that resolves to whatever is newest gets no verdict, and pm refuses the
build file before it runs.

Untested beyond evaluation and the payload builder itself: both architectures
evaluate `.#pm-payloads.<name>`, and the builder, run against nixpkgs' own
cached static packages, produced a static `hello` and refused `gnugrep`. No pm
build has run the steps above, and CI does not build any payload, so the first
one of each package compiles its whole static closure.

## Virtual machines

The image runs no hypervisor daemon. `nixos/modules/quickemu.nix`
([Virtual machine desktop](hypervisor.md)) installs quickemu and gives the
machines a network, and each person runs their own machines as
themselves, with KVM where the CPU has it, without asking anyone.

pm boots a package that ships its own kernel (`kernel(...)` in its recipe, see
pm's README) in a virtual machine, starting QEMU itself, as you:

```
pm run hello.cpkg
```

The guest is what it is under QEMU: the package and the libraries its
programs link, no network device and no disk. Only x86-64 boots a
package's kernel. pm's `quickemu` plugin declines such a boot, and pm's
`libvirt` plugin, which handed it to libvirt through `virsh create`, is not
in the image any more: there is no libvirt for it to drive.

pm's own QEMU boot of a distribution kernel running a probe, under TCG,
ran on an Ubuntu host, as did the same boot through the libvirt plugin
while the image still carried libvirt. Neither has run on this image yet.

### Machines that stay

`pm machine` keeps virtual machines that outlive any one command, the
machines of the [virtual machine desktop](hypervisor.md). pm leaves them to
its `quickemu` plugin, which turns each request into the steps that carry
it out, held to the programs `pm plugins` lists under its `launchers:`
(`cp`, `qemu-img`, `quickemu`, `systemd-run`, `systemctl` and a few
more). Like every plugin it has to be installed and signed
([above](#pms-plugins)); derisk's Machines app offers to do that for you.

```
pm machine create work \
  --image /run/current-system/sw/share/losos/machines/losos-guest.qcow2 \
  --label blue --memory 4096 --cpus 4 --disk 64
pm machine start work
pm machine list
pm machine stop work        # asks the guest to shut down
pm machine kill work        # pulls the plug
pm machine remove work      # the machine and its disk; the template stays
```

A machine is yours, as your files are: everything of it is under
`~/.local/share/pm/machines`, which pm keeps private to you, and it runs as
you. The template is copied in read-only the first time, as
`templates/<name>-<digest>.qcow2`, so every machine of yours made from one
image shares it, and a changed image is a new template beside the old one.
The machine is a folder, `<name>/`, holding its disk, `disk.qcow2`, a qcow2
layer on the template as large as `--disk` says and never smaller than the
template, and pm's record of it, `machine.json`, with its label, its
template and its vsock address; beside the folder is `<name>.conf`, the
settings quickemu reads. `start` runs `quickemu --vm <name>.conf` as a
service of your own systemd user manager, `pm-machine-<name>`, through
`systemd-run --user`; `stop` has quickemu say `system_powerdown` on QEMU's
monitor; `kill` stops the service; `list` names the folders and the
services that run. derisk's Machines app reads the same folders and starts
a machine the same way.

`--installer CD.iso` (more than once for more CDs) makes an empty machine
that installs its system from them instead, and `--system windows` makes it
a Windows PC; pm adds the image's CD of guest tools for that system, from
`share/pm/machines` on `XDG_DATA_DIRS`, or the one `--tools` names
([Machines from an installer CD](hypervisor.md#machines-from-an-installer-cd)).
quickemu takes two CDs at most, the installer's and one of tools. Each is
copied in read-only under `cds/` once and shared like a template. Removing
a machine deletes its folder and its settings, never a template or CD
another machine may share.

`nixos/tests/machines.nix` makes a machine from the image's own template
this way, under TCG, and checks that quickemu boots it, that it grows its
root over its 8 GiB disk, gets an address on the machines' network, lists
its apps through the agent, and that opening one reaches the host's vsock
port 7350 from the address in pm's record. The test runs as root, so its
machines are root's and run as system services. CI runs it with the other
VM tests.
