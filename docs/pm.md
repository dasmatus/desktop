# pm on LosOS

## pm's plugins

`nixos/pkgs/pm-plugins.nix` builds seven components, this repository's three
and pm's own `libvirt`, `sysext`, `sysupdate` and `systemd`, and the image
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

The image runs libvirt from boot (`nixos/modules/libvirt.nix`, [Virtual
machine desktop](hypervisor.md)): QEMU for the machine's own architecture,
with KVM where the CPU has it. virt-manager and `virsh` use it as on any
distribution. `LIBVIRT_DEFAULT_URI` is `qemu:///system` for every session.
Administrators, the `wheel` members first-boot setup makes, may manage
machines there without being asked; everyone else is asked for an
administrator's password.

pm boots a package that ships its own kernel (`kernel(...)` in its recipe, see
pm's README) in a virtual machine. On its own it starts QEMU itself, as you.
With pm's `libvirt` plugin installed and signed (as above; it is one of the
seven), it asks libvirt instead:

```
pm run hello.cpkg          # through libvirt, when the plugin is installed
pm run --qemu hello.cpkg   # QEMU directly, as you, asking no plugin
```

The plugin is a WebAssembly component like the rest, and it can reach nothing
itself: it writes a transient domain definition and names one program,
`virsh`, which `pm plugins` lists under `launchers:`, and pm runs
`virsh create --console --autodestroy` with it. virsh connects where it always
does, `LIBVIRT_DEFAULT_URI`, which here is `qemu:///system`. There QEMU runs as
libvirt's own user rather than as you, in its own cgroup and mount namespace
with QEMU's seccomp sandbox on, and libvirt hands it the kernel, the
initramfs and the two serial sockets for the life of the machine.
`virsh list` and virt-manager show the machine as `pm-vm-<random>`, titled
`pm run: <program>`, while it runs. It is destroyed when virsh's connection
closes, and pm stops virsh when pm dies, so a killed pm takes its machine with
it, and Ctrl-C stops it and gives the terminal back. The guest is what it is
under QEMU: the package and the libraries its programs link, no network device
and no disk. Only x86-64 boots a package's kernel.

The real boot through the plugin (a distribution kernel running a probe
through `qemu:///system`, QEMU as libvirt's own user, under TCG) passed on an
Ubuntu host with libvirt 10.0, as did the same boot with pm's own QEMU. Neither
the module nor the plugin has run on this image yet.

### Machines that stay

`pm machine` keeps virtual machines that outlive any one command, the
machines of the [virtual machine desktop](hypervisor.md). As with `pm run`,
pm leaves libvirt to its `libvirt` plugin, which turns each request into the
`virsh` steps that carry it out:

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

The template becomes a read-only volume in libvirt's `default` pool, named
after it and its digest, so every machine made from one image shares it. A
machine's disk is a qcow2 overlay on it (`pm-machine-<name>.qcow2`), as
large as `--disk` says and never smaller than the template, and its
definition is a persistent domain that carries its label and template in
`<metadata>`, a vsock device and a channel for the QEMU guest agent. derisk's
Machines app runs these same commands.

`nixos/tests/machines.nix` makes a machine from the image's own template
this way, under TCG, and checks that it boots, grows its root over its
8 GiB disk, gets an address on libvirt's network, lists its apps through the
agent, and that opening one reaches the host's vsock port 7350 from the
address libvirt gave the machine. CI runs it with the other VM tests.
