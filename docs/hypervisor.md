# Virtual machine desktop

On a PC, LosOS Desktop is a hypervisor with a desktop on top, in the mould
of [Qubes OS](https://www.qubes-os.org). Linux with KVM runs the machines:
the kernel schedules each guest straight onto the processor's
virtualisation extensions, with no other operating system under it, which
makes it a type-1 hypervisor the way Xen is under Qubes. The host keeps
only what it takes to drive the hardware and the desktop: the kernel,
systemd, quickemu, derisk and the host's own settings. The apps a person
works in are meant to run in machines, each one labelled with a colour that
marks its windows on the desktop. This page describes what is built so far:
the host, the machine template it ships, and how the desktop shows and runs
machines.

A machine is its owner's files. [quickemu](https://github.com/quickemu-project/quickemu)
starts each one from a short settings file and leaves, and nothing in the
OS runs between a person and their machines or keeps a list of them
elsewhere: pm and derisk read and write the same folder, and start a
machine the same way.

libvirt ran the machines until quickemu took its place. Its daemon was a
second record of every machine beside the files that are the machine,
shared by every account and reached through polkit, so anyone but an
administrator was asked for an administrator's password to start a
machine of their own.

The GSI does not do any of this. Most phones' kernels have no KVM, so apps
there keep running on the host ([Halium GSI](halium.md)).

## The host

`nixos/modules/quickemu.nix` makes the PC the host. Nothing in it is per
machine or per person:

- **quickemu is installed**, and runs QEMU with KVM where the processor has
  it. A PC without hardware virtualisation gets the same image; quickemu
  falls back to emulating, and machines run, slowly. Its OVMF firmware is
  built with TPM support (`nixos/pkgs/default.nix`), since the one it would
  otherwise boot without Secure Boot never measures what it boots, and
  Windows 11 refuses a TPM like that ([Trying it in quickemu](quickemu.md)).
  udev's default rules leave `/dev/kvm` and `/dev/vhost-vsock` open to
  every account, so a person's own QEMU uses both.
- **The machines' network is `virbr0`**, `192.168.122.0/24`, the bridge,
  address and NAT libvirt used to make, now networkd's: the host is
  `192.168.122.1` and masquerades what the machines send out. A dnsmasq of
  the OS's own, `losos-machines-dns`, gives the machines addresses from
  `.2` to `.254` and answers their DNS ([below](#names-on-the-network)).
  The firewall accepts DHCP and DNS from the machines on `virbr0` and
  nothing else.
- **QEMU's bridge helper is setuid**, at
  `/run/wrappers/bin/qemu-bridge-helper`, as NixOS made it for libvirt. It
  makes a machine's tap device and adds it to `virbr0`, which takes a
  privilege the person running the machine does not have, and
  `/etc/qemu/bridge.conf` lets it add taps to `virbr0` and no other bridge.
  quickemu's QEMU runs it from there (`nixos/pkgs/default.nix`). A
  machine's settings fall back to QEMU's own user networking on a host
  without `virbr0`.
- **`vhost_vsock` is loaded at boot.** It is the host's end of each
  machine's virtio-vsock device, and the transport the desktop listens on
  for the machines' windows.
- **waypipe is installed.** The desktop runs `waypipe client` for each
  running machine to turn what comes out of the guest back into Wayland.
- **remote-viewer and WinApps are installed** for machines that install
  from a CD ([below](#machines-from-an-installer-cd)): remote-viewer shows
  a machine's own screen, and WinApps opens a Windows machine's apps as
  windows of their own. quickemu starts swtpm, the TPM a Windows machine
  is given, itself. WinApps only on x86-64, where Windows machines are.

Nobody is asked for a password to manage a machine: a person's own
machines, run as them, need nobody's permission.

A machine is state in its owner's home, and the machines' network keeps
its leases under `/var/lib/losos-machines-dns` on the root partition. The
verity `/usr` and the [update model](systemd.md) are unchanged.

## Machines

pm makes them (`pm machine create`, [pm on LosOS](pm.md#machines-that-stay)),
and its quickemu plugin is the only part of pm that knows quickemu exists.
Every person's machines are their own, under `~/.local/share/pm/machines`
(`pm/machines` under `$XDG_DATA_HOME`), which pm keeps private to them. A
machine is:

- its **template**, `templates/<name>-<digest>.qcow2`, a read-only copy of
  the image it was made from, copied in the first time one of the person's
  machines is made from it and shared by every one of theirs made from the
  same bytes;
- its **folder**, `<name>/`: its disk, `disk.qcow2`, a qcow2 layer on the
  template holding only what the machine has changed; pm's record of it,
  `machine.json`, with its name, UUID, label, template, memory, CPUs and
  vsock address; and what quickemu keeps there itself, the firmware's
  variables and, while it runs, QEMU's pid file and the sockets of its
  monitor (`<name>-monitor.socket`), its guest agent (`<name>-agent.sock`)
  and its screen (`<name>.sock`);
- its **settings**, `<name>.conf` beside the folder, the shell variables
  quickemu reads: UEFI without Secure Boot, a virtio disk, a network card
  on `virbr0` with a MAC address pm drew at random, a vsock device at the
  address pm drew for it, no sound card, no shared folder and no GL.
  quickemu gives every machine a screen, on SPICE's Unix socket in its
  folder and never on a port, and starts no viewer for it.

A machine runs as a transient service of its owner's systemd user manager,
`pm-machine-<name>`: `systemd-run --user` starts it around `quickemu --vm
<name>.conf`, quickemu starts QEMU and leaves, and the service, which
follows QEMU's pid file, lasts as long as QEMU does. Whether that service
is active is whether the machine runs. Root's machines are under `/root`
and run as services of the system's manager.

derisk's Machines app lists them by reading the same folders, makes new
ones from the templates under `share/losos/machines` (through `pm machine
create`), and starts, shuts down, forces off and removes them. It starts a
machine as pm does, the same service in the same way, so each sees what
the other started; it shuts one down by saying `system_powerdown` on QEMU's
monitor and forces it off with `quit`. For a running machine it also lists
the apps the machine has and opens the one picked. Until the person has
pm's quickemu plugin installed and signed, the app offers to do it, with
their own key ([pm's plugins](pm.md#pms-plugins)).

## Names on the network

A machine looks up `.local` names through the host instead of multicasting
for them itself. The host's NAT carries no multicast between `virbr0` and
the host's own networks, so an mDNS query a machine sent would reach only
the other machines on `virbr0`, never a LosOS box, printer or other PC on
the host's LAN. The host is on both:

1. The guest's resolved does no mDNS or LLMNR, and NetworkManager's
   profile for its link, `machines`, gives that link `~local` as a routing
   domain (`ipv4.dns-search`), so a `.local` lookup goes to the DNS
   server DHCP gave it, which is the host (`192.168.122.1`). resolved never
   sends a `.local` name to a unicast server otherwise.
2. The machines' dnsmasq on `virbr0`, `losos-machines-dns`, answers it
   from the host's own resolver, resolved's stub at `127.0.0.53`.
3. resolved on the host asks every link it does mDNS on, the LAN and
   `virbr0` alike, and answers with the address it found.

The machine then reaches that address through the host's NAT like any
other. A LosOS box's name is the exception while a LosOS boxes machine
runs: dnsmasq answers it with that machine's address, and the connection
goes to the box through it ([from the other machines](losos-boxes.md#from-the-other-machines)). With nothing multicasting in the machine, it also neither answers
nor announces on `virbr0`: the host's answer is the only one. A machine
installed from a CD keeps whatever its own system does, which for most
distributions is Avahi multicasting on `virbr0` alone.

The host finds the machines the other way round. Each machine names itself
in its DHCP request, and dnsmasq answers that name under `.machines`
(`fedora.machines`) for itself, never passing such a question on; the
host's resolved sends `.machines` names to it and nothing else. That is
how WinApps reaches a Windows machine ([Windows](#windows)).

`nixos/tests/machines.nix` checks it: a machine made from the template
finds `lanbox.local`, a test machine on the host's LAN that answers only
over mDNS, at that machine's LAN address, and pings it, and the host finds
that machine by its own name under `.machines`.

## A machine's windows

A machine has no screen of its own that anyone looks at. Its apps' windows
come to the host's desktop over virtio-vsock instead, and the desktop
knows which machine each one is from by where it came in, never by
anything the guest says:

1. In the guest, an app runs under `waypipe --vsock -s 2:7350 server`, which
   carries its Wayland connection to the host (vsock address 2), port 7350.
2. The desktop session listens on that port. The kernel stamps each
   connection with the address the sending machine's QEMU was given, and
   the session looks for the running machine whose record has that
   address. pm draws a machine's address at random when it makes it; the
   kernel lets no two running machines share one, and QEMU refuses to
   start a machine whose address another has taken. A stopped machine
   does not count, since its record may name an address another machine
   has now.
3. It hands the connection to that machine's `waypipe client`, which
   connects to a Wayland socket the compositor opened for that machine
   alone, under `$XDG_RUNTIME_DIR/derisk/machines`.
4. The compositor marks every client of that socket with the machine's
   name, through the same security-context mechanism Flatpak's sandbox uses,
   so the machine's windows get none of the privileged protocols a host app
   may use (reading other windows, placing itself over them, synthetic
   input).
5. Windows have no title bars of their own: the top bar shows the focused
   window's title and buttons. derisk fills the top bar with the machine's
   label colour while one of its windows has the focus, draws a border in
   that colour round each of its windows, focused or not, and puts the
   machine's name before its title; the top bar also shows a dot in that
   colour for each running machine.

Apps start in a machine through the QEMU guest agent: derisk asks the agent,
on its socket in the machine's folder, to run the guest's `losos-guest-run`
with the app's desktop file ID, and that starts it as the guest's user
under waypipe.

## On the desktop

Besides the Machines app, machines are a step away wherever the desktop
starts things:

- **The command palette** offers "Start work" for each machine that is off,
  "Terminal in work" for each app it has, and "Shut down work" while it
  runs. A stopped machine's apps are offered too, as the machine last
  listed them or, for one never started, as its template's app list says
  ([below](#the-guest-template)); picking one starts the machine and opens
  the app once its guest agent answers. A machine's label colour is among its search words, so typing
  "blue" finds the machine whose windows are blue. The rows come from a
  palette plugin like the others, fed by the session itself; no other
  connection to derisk can speak for it.
- **The overview** has a Machines card: how many are running, a start or
  shut down button for each, and a way into the Machines app.
- **Apps can open in a machine by default.** With "Open apps in work"
  ticked in the Machines app, an app started from the app grid or the
  palette opens in that machine instead of on the host, by the same desktop
  file ID, starting the machine first when it is off. derisk's own core apps
  (Files, Settings and the rest) stay on the host, and so does everything
  once that machine has been removed. An app the machine does not have
  opens nowhere; the session's log says why.
- **A machine's own screen gets a window.** While a machine with a screen
  runs (one installing from a CD, or Windows), the session shows it in
  remote-viewer on that machine's SPICE socket,
  `spice+unix://<folder>/<name>.sock`, as a client of the machine's own
  Wayland socket, so the window is framed in its colour like its apps'
  windows. Closing it hides the screen until the machine starts again or
  the palette's "Show win's screen" brings it back.
- **Keep on Top** is in a window's menu and the palette, for any window but
  an untrusted (red) machine's. Such a window cannot be kept on top, and one
  that asks to be raised gets attention instead of focus, so a machine
  meant for untrusted work cannot cover a window to imitate it.

## The guest template

The image ships three templates under `share/losos/machines` on
`XDG_DATA_DIRS`, in `/usr` and covered by its verity hash like the rest of
the OS: the LosOS guest, `losos-guest.qcow2`, for a person's own apps; the
[LosOS boxes machine](losos-boxes.md#the-losos-boxes-machine),
`losos-boxes.qcow2`, the one machine that talks to LosOS boxes; and
[the Flatpak machine](#the-flatpak-machine), `losos-flatpak.qcow2`, where
Flatpak apps run. All three are the same machine, `nixos/guest/common.nix`,
with different apps: `nixos/guest/default.nix` adds the Calculator,
`nixos/guest/boxes.nix` adds `losos` and box discovery, and
`nixos/guest/flatpak.nix` adds Flatpak and Flathub.
Beside each template is `<name>.apps`, what `losos-guest-run --list`
prints in it, written when the image is built, so the desktop can offer a
machine's apps before it has ever started.
`nixos/modules/machines.nix` evaluates each with the image's own package
set and turns it into its template. They are the option
`losos.machines.guests`, a NixOS module per template name, so a part of
the OS that needs a machine of its own adds one there, as
`nixos/modules/losos-boxes.nix` adds the LosOS boxes machine. An update
brings new templates; machines made from the old ones keep them, since
pm copied each into its owner's machines.

A part of the OS that runs in a machine can also have every person given
that machine, with `losos.machines.system.<name> = { template; label; }`
(and `memory` and `disk` where pm's defaults do not do).
`losos-system-machines`, a service of each person's own systemd user
manager, makes each one in their machines when they sign in, as `pm
machine create` would, once: it keeps the names it made under
`~/.local/state/losos/system-machines`, so a machine its owner removed in
the Machines app is not made again. pm runs its quickemu plugin only when
a key its user trusts signed it, and the person may have set up no key or
plugin, so the unit keeps a pm configuration of its own there, with a key
made on that computer the first time and never in the image, and signs
the image's plugin with it at every start, since an update may bring a
new one. A system service made these machines once, at boot, while
libvirt kept machines for everyone; a machine in root's home would be
nobody's to open.

What follows is the LosOS guest.

The guest boots its UKI from the ESP's fallback path, with no boot loader,
and its root is one ext4 partition holding the Nix store and its state,
made as small as the store allows. A machine's disk is as large as
`pm machine create --disk` says, 32 GiB unless told otherwise, and on its
first boot systemd-repart in the initrd grows the root partition over the
rest of it and ext4 grows with it. The qcow2 holds only what the build
wrote (0.96 GB for the reduced build tested below, more with the apps it
left out). It has:

- **the QEMU guest agent**, through which the host starts apps;
- **one account, `user`**, with no password, whose systemd manager runs from
  boot (lingering). Nobody logs in: its apps run under that manager, which
  makes the folders apps expect in its home as the host's users' do
  (`nixos/modules/home-folders.nix`: the XDG base and user directories with
  `user-dirs.dirs`, and the fonts, icons and derisk folders Danube shows);
- **a waypipe server** in that manager, `waypipe --vsock --socket 2:7350
  --no-gpu --display wayland-0 server`. Every app the user runs finds it as
  `WAYLAND_DISPLAY=wayland-0`, and each app's connection becomes a vsock
  stream to the host's desktop, port 7350. There is no GPU in a machine, so
  apps draw with Mesa's llvmpipe and lavapipe into shared memory, which is
  what waypipe carries;
- **apps**: derisk's Terminal (`derisk-terminal`, drawing in software, as
  there is no GPU), Danube, derisk's Calculator (the one derisk core app that
  is a window of its own, `derisk-gpui`; the rest are drawn by the host's
  compositor). Flatpak is not in it but in [its own
  machine](#the-flatpak-machine);
- **NetworkManager**, taking an address from the machines' network and
  checking it reaches the internet against GNOME's
  `nmcheck.gnome.org`, which apps in the machine ask through GLib or the
  portal before they go online. resolved answers DNS, and asks the host
  for `.local` names ([above](#names-on-the-network)). The host keeps
  networkd ([systemd](systemd.md)).

`losos-guest-run`, in the guest's system profile, is what the host runs
there through the agent:

- `losos-guest-run --list` prints one line per app the desktop can open:
  its desktop file ID, a tab and its name. It reads the user's own desktop
  files first, then the user's and the system's Flatpak exports, then the
  system's, and leaves out entries that are hidden, need a terminal or are
  not applications.
- `losos-guest-run <desktop file ID>` starts that app's `Exec=` line,
  without its field codes, in a login shell as a transient unit of the
  user's manager (`systemd-run --user --machine=user@.host`), and returns
  once it has started.

Booted from the template under libvirt with TCG, before quickemu took its
place (no KVM, no network and no vsock device), a reduced build of the
guest without Danube, Flatpak and the Calculator answered the agent within
two minutes of start, its root grown from the template's 3.3 GiB to fill
an 8 GiB disk. `--list` listed foot's three desktop entries, and
`losos-guest-run foot` started foot in the user's manager, where it
connected to the waypipe server; waypipe then failed to reach the host,
because that machine had no vsock device.

## The Flatpak machine

On a PC, Flatpak apps run in a machine of their own, `flatpak`, labelled
purple, and nothing of Flatpak is on the host. `nixos/modules/flatpak-machine.nix`
adds its template, `losos-flatpak` (`nixos/guest/flatpak.nix`), and has
each person's made in their machines when they sign in
(`losos.machines.system`, above). The template is the LosOS guest's
machine with Flatpak and Flathub, the same `flatpak.nix` a phone runs on
its host ([Halium GSI](halium.md)); its user may install Flatpaks for the
whole machine without being asked. It has no store of its own: the host's
[Store](store.md) installs into it. A Flatpak's own
sandbox still runs inside it, so an app is kept apart from the machine's
other apps as before, and the machine keeps all of them apart from the
host.

- **On the desktop** every installed app is that machine's app: "Maps in
  flatpak" in the palette and the app grid, framed in purple. They are
  offered while the machine is off too, from the last list it gave, and
  opening one starts it.
- **The host's Store** installs, removes and updates apps in it. derisk
  finds the machine by its template and asks its `losos-guest-flatpak`
  through the guest agent: `store run list|remote-ls ...` answers as the
  guest's user does, `store start install|uninstall|update ...` starts
  `flatpak` in a unit of that user's manager and returns at once, and
  `store progress` gives back what the job has printed so far and, once it
  has ended, its exit status, which the Store reads a few times a second
  for its progress bar. The agent hands a program's output over only once
  it has exited, so this is how an install shows progress at all. Flathub's
  catalogue and icons are read from the machine's disk with the agent's
  file calls, in 4 MiB pieces, as they are far larger than what it keeps of
  a program's output.
- **Settings' Privacy page** lists the Flatpak apps the machine has and
  changes their permissions there. derisk asks the guest's
  `losos-guest-flatpak` through the guest agent: `apps` reads each app's
  metadata and the user's overrides, `overrides <app>` writes them from
  standard input, and `flatpak permission-show`, `permission-set` and
  `permission-remove` run as the guest's user, in whose session the
  portals' permission store is. While the machine is off the page says so
  and offers to start it; nothing is read from it until it runs.
- **Apps that were on the host move in once.** A PC that updates from a
  release with Flatpak on the host has its apps where the host kept them.
  At each person's next sign-in, once their Flatpak machine is made, a
  service of their user manager, `losos-flatpak-move`, moves each of three
  things into it once: `/var/lib/flatpak` (the apps installed for
  everyone), copied into every person's machine in turn and so left on
  the host ([What is not done](not-done.md)); their `~/.local/share/flatpak` (apps installed for themselves), deleted
  from the host once moved; and `~/.var/app` (every app's data), copied to
  the machine's user and left on the host for its owner to delete. A mark
  under `~/.local/state/losos` says each was moved. Each move,
  `losos-flatpak-move-host`, makes the files an ext4 disk under `/var/tmp`,
  boots the machine once from a copy of its settings that adds the disk,
  read-only, with the serial `losos-flatpak-move`, has the guest's
  `losos-flatpak-move` copy it where it belongs through the guest agent,
  shuts the machine down through QEMU's monitor and deletes the disk. The
  machine must be off for it, since the disk is added to its boot: a move
  that finds it running waits for the next sign-in. A system service moved
  `/var/lib/flatpak` once, into the one Flatpak machine libvirt kept for
  everyone, and only administrators' own files moved, since anyone else
  would have been asked for a password; every person has a machine of
  their own now and moves into it without asking.

## Machines from an installer CD

A machine can also start empty and install a system from its own CD, as a
PC would:

```sh
pm machine create win --installer Win11_24H2_English_x64.iso --system windows --label red
pm machine create fedora --installer Fedora-Workstation-Live-43.iso --label green
```

Each CD is copied read-only into the person's machines under `cds/`,
named after it and its digest, and shared by every machine of theirs given
the same bytes; the machine's disk starts empty. quickemu boots the first
CD while the disk looks unused and leaves it out once something is
installed, so the installer runs once and the installed system starts after
it; the second CD stays in its drive. A machine takes two CDs at most, the
installer's and one of tools. Such a machine has a screen, which pm
records in `machine.json`: SPICE on its Unix socket, never on a port.
quickemu gives it a USB tablet, puts its CDs on SATA on x86-64 and on
virtio-scsi on aarch64, and always adds SPICE's agent channel, which
carries the clipboard to a guest that runs `spice-vdagent`. The desktop
shows the screen in a window of its own (see "On the desktop"); outside a
session, `remote-viewer spice+unix://$HOME/.local/share/pm/machines/win/win.sock`
does.

pm also gives it the image's **CD of guest tools** for its system, which
the image ships at `share/pm/machines/<system>-tools.iso` where pm looks
for it (`nixos/modules/machines.nix`, sources in `nixos/guest-tools/`).

### Windows

A Windows machine is what Windows 11 asks of a PC: UEFI, a TPM 2.0 (swtpm,
which quickemu starts beside it, its state kept in the machine's folder
and deleted with it), Hyper-V's enlightenments when the host has KVM, its
clock in local time, 4096 MiB and 64 GiB unless told otherwise. Its
settings say `guest_os='windows'` and `tpm='on'`, with the Windows CD as
quickemu's `iso` and the drivers CD as its `fixed_iso`. It has no vsock,
which Windows has no use for. x86-64 only.

The Windows tools CD carries virtio's drivers and an answer file,
`autounattend.xml`, which Windows Setup reads from the root of any drive it
starts with:

1. Setup loads virtio's disk and network drivers (`viostor`, `NetKVM`, for
   Windows 10 and 11) from the CD and adds them to the Windows it installs,
   which it otherwise could not see a disk to install on.
2. It passes over Windows 11's Secure Boot, TPM, memory and processor
   checks: the machine boots without Secure Boot, since its firmware has no
   Microsoft keys to check Windows' boot loader with.
3. It wipes the machine's one disk and lays it out as Windows lays out a
   UEFI PC.
4. It offers a local account as well as a Microsoft one.
5. At the first sign-in, `losos\setup.cmd` from the CD installs the rest
   of virtio's drivers and the QEMU guest agent (virtio-win's guest tools),
   then runs WinApps' own Windows setup: Remote Desktop on, RemoteApp for
   every program, and its firewall rule.

What Setup still asks: the language, the product key or edition, and the
account. quickemu answers the firmware's "Press any key to boot from CD"
itself, pressing Enter for the first few seconds of a start while the
Windows CD is in; if that misses it, the prompt is the person's to answer
on the machine's screen, and a machine left at the firmware's boot menu
offers it again when started again.

**WinApps** ([winapps-org/winapps](https://github.com/winapps-org/winapps),
`nixos/pkgs/winapps.nix`) then opens the machine's Windows apps as windows
of their own on the desktop, each a FreeRDP RemoteApp session. It is set up
as its own documentation says for its manual flavour, in
`~/.config/winapps/winapps.conf`: `WAFLAVOR="manual"`, `RDP_IP` the
machine's computer name followed by `.machines` (`DESKTOP-ABC123.machines`:
the name Windows gave the machines' DHCP, which Settings > System > About
shows; [Names on the network](#names-on-the-network)), and `RDP_USER` and
`RDP_PASS` the Windows account; then `winapps-setup --user` lists the
machine's apps into the app grid. The machine has to be running for them
to open. Remote Desktop needs Windows' Pro edition or above. WinApps'
libvirt flavour, which found the machine through `virsh`, went with
libvirt.

### Other distributions

Most distributions' installers already have virtio's drivers. What they
lack is what fits them to this desktop, which the Linux tools CD's
`setup.sh` adds. Run it once in the installed system, as root:

```sh
sudo sh /run/media/$USER/LOSOS_TOOLS/setup.sh
```

It keeps virtio's disk, network and vsock drivers in the initramfs
(through dracut, initramfs-tools or mkinitcpio, whichever the distribution
has), installs and starts the QEMU guest agent and waypipe with the
distribution's own package manager (apt, dnf, zypper or pacman), runs a
waypipe server over vsock for the user who ran it, as the LosOS guest does,
and installs `losos-guest-run`. After that the desktop lists and opens the
machine's apps like a LosOS guest's, framed in its colour. Its socket is
`losos-wayland`, not `wayland-0`, so a desktop the distribution runs on the
machine's own screen keeps its own.

The machines VM test (`nixos/tests/machines.nix`) makes, without KVM, a
Windows machine from a stand-in CD, checks that its settings give it the
drivers CD, a TPM and a screen, starts it and waits for its swtpm and its
screen's socket, and removes it, leaving both CDs for other machines. No
Windows or other distribution has been installed from them yet.
