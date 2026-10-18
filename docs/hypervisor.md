# Virtual machine desktop

On a PC, LosOS Desktop is a hypervisor with a desktop on top, in the mould
of [Qubes OS](https://www.qubes-os.org). Linux with KVM runs the machines:
the kernel schedules each guest straight onto the processor's
virtualisation extensions, with no other operating system under it, which
makes it a type-1 hypervisor the way Xen is under Qubes. The host keeps
only what it takes to drive the hardware and the desktop: the kernel,
systemd, libvirt, derisk and the host's own settings. The apps a person
works in are meant to run in machines, each one labelled with a colour that
marks its windows on the desktop. This page describes what is built so far:
the host, the machine template it ships, and how the desktop shows and runs
machines.

[libvirt](https://libvirt.org) is the one control plane. Nothing in the OS
starts QEMU itself or keeps its own list of machines: pm, virsh and
virt-manager all talk to the same daemon, `qemu:///system`, and see the same
machines.

The GSI does not do any of this. Most phones' kernels have no KVM, so apps
there keep running on the host ([Halium GSI](halium.md)).

## The host

`nixos/modules/libvirt.nix` makes the PC the host:

- **libvirtd runs from boot**, with QEMU for the PC's own architecture and
  KVM where the processor has it. A PC without hardware virtualisation gets
  the same image; libvirt reports that its QEMU emulates rather than runs on
  KVM, and machines run, slowly.
- **`LIBVIRT_DEFAULT_URI` is `qemu:///system`** for every session, so
  `virsh`, pm and virt-manager reach the machines without being told where
  they are. A per-user `qemu:///session` would have none of them, and could
  not give a machine the vsock its windows travel over, because
  `/dev/vhost-vsock` is root's.
- **The `default` storage pool and network exist and start with the host.**
  The pool is a directory, `/var/lib/libvirt/images`; the network is
  libvirt's own NAT on `virbr0`, `192.168.122.0/24`. A oneshot unit,
  `libvirt-defaults`, defines and starts them through libvirt's API at boot,
  so a change made to them in virt-manager stays. The firewall accepts DHCP
  and DNS from the machines on `virbr0` and nothing else; libvirt's own
  nftables table masquerades their traffic out.
- **`vhost_vsock` is loaded at boot.** It is the host's end of each
  machine's virtio-vsock device, and the transport the desktop listens on
  for the machines' windows.
- **waypipe is installed.** The desktop runs `waypipe client` for each
  running machine to turn what comes out of the guest back into Wayland.
- **swtpm, virt-viewer and WinApps are installed** for machines that install
  from a CD ([below](#machines-from-an-installer-cd)): swtpm is the TPM
  libvirt gives a Windows machine, virt-viewer shows a machine's own screen,
  and WinApps opens a Windows machine's apps as windows of their own.
  WinApps only on x86-64, where Windows machines are.
- **Administrators manage machines without being asked.** polkit lets the
  `wheel` group, which first-boot setup puts the first account in, use
  `org.libvirt.unix.manage` outright: handing a machine a host disk is an
  administrator's power, and the desktop, pm and virt-manager connect many
  times a session. Everyone else is asked for an administrator's password
  by polkit's agent. A read-only connection (which machines exist and run,
  their labels, which vsock address is whose) is open to every account, so
  every session can frame machines' windows.

All of it is state under `/var/lib/libvirt` and `/run/libvirt` on the root
partition. The verity `/usr` and the [update model](systemd.md) are
unchanged.

## Machines

pm makes them (`pm machine create`, [pm on LosOS](pm.md#machines-that-stay)),
and its libvirt plugin is the only part of pm that knows libvirt exists.
A machine is three things in libvirt:

- its **template**, a read-only volume in the `default` pool that every
  machine made from it shares;
- its **disk**, a qcow2 overlay on the template in the same pool, holding
  only what the machine has changed;
- its **definition**, a persistent domain with UEFI, a virtio disk and
  network card on `default`, a channel for the QEMU guest agent, a vsock
  device whose address libvirt picks, no graphics card and no USB
  controller, and in its `<metadata>` the machine's label and template.

derisk's Machines app lists them, makes new ones from the templates under
`share/losos/machines` (through `pm machine create`), and starts, shuts down
and removes them. For a running machine it also lists the apps the machine
has and opens the one picked.

## Names on the network

A machine looks up `.local` names through the host instead of multicasting
for them itself. libvirt's NAT carries no multicast between `virbr0` and
the host's own networks, so an mDNS query a machine sent would reach only
the other machines on `virbr0`, never a LosOS box, printer or other PC on
the host's LAN. The host is on both:

1. The guest's resolved does no mDNS or LLMNR, and NetworkManager's
   profile for its link on `default` gives that link `~local` as a routing
   domain (`ipv4.dns-search`), so a `.local` lookup goes to the DNS
   server DHCP gave it, which is the host (`192.168.122.1`). resolved never
   sends a `.local` name to a unicast server otherwise.
2. libvirt's dnsmasq on `virbr0` answers it from the host's own resolver,
   `/etc/resolv.conf`, which is resolved's stub.
3. resolved on the host asks every link it does mDNS on, the LAN and
   `virbr0` alike, and answers with the address it found.

The machine then reaches that address through libvirt's NAT like any
other. A LosOS box's name is the exception while a LosOS boxes machine
runs: dnsmasq answers it with that machine's address, and the connection
goes to the box through it ([from the other machines](losos-boxes.md#from-the-other-machines)). With nothing multicasting in the machine, it also neither answers
nor announces on `virbr0`: the host's answer is the only one. A machine
installed from a CD keeps whatever its own system does, which for most
distributions is Avahi multicasting on `virbr0` alone.

`nixos/tests/machines.nix` checks it: a machine made from the template
finds `lanbox.local`, a test machine on the host's LAN that answers only
over mDNS, at that machine's LAN address, and pings it.

## A machine's windows

A machine has no screen. Its apps' windows come to the host's desktop over
virtio-vsock instead, and the desktop knows which machine each one is from
by where it came in, never by anything the guest says:

1. In the guest, an app runs under `waypipe --vsock -s 2:7350 server`, which
   carries its Wayland connection to the host (vsock address 2), port 7350.
2. The desktop session listens on that port. The kernel stamps each
   connection with the address libvirt gave the sending machine, and the
   session asks libvirt which machine has that address now.
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

Apps start in a machine through the QEMU guest agent: derisk asks libvirt to
run the guest's `losos-guest-run` with the app's desktop file ID, and that
starts it as the guest's user under waypipe.

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
  virt-viewer on that machine's socket, so the window is framed in its
  colour like its apps' windows. Closing it hides the screen until the
  machine starts again or the palette's "Show win's screen" brings it back.
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
brings new templates; machines made from the old ones keep them.

A part of the OS that runs in a machine can also have the host make that
machine, with `losos.machines.system.<name> = { template; label; }` (and
`memory` and `disk` where pm's defaults do not do). `losos-system-machines`
makes each one at boot, as `pm machine create` would, once: it keeps the
names it made under `/var/lib/losos-machines`, so a machine its owner
removed in the Machines app is not made again. pm runs its libvirt plugin
only when a key its user trusts signed it, so the unit keeps a key of its
own there, made on that computer at its first boot and never in the image,
and signs the image's plugin with it at every start.

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
- **NetworkManager**, with NixOS's defaults as on the host, taking an
  address from libvirt's `default` network. Apps in the machine ask it
  through GLib or the portal whether they are online. resolved answers
  DNS, and asks the host for `.local` names
  ([above](#names-on-the-network)).

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

Booted from the template under libvirt with TCG (no KVM, no network and no
vsock device), a reduced build of the guest without Danube, Flatpak and
the Calculator answered the agent within two minutes of start, its root
grown from the template's 3.3 GiB to fill an 8 GiB disk. `--list` listed
foot's three desktop entries, and `losos-guest-run foot` started foot in
the user's manager, where it connected to the waypipe server; waypipe then
failed to reach the host, because that machine had no vsock device.

## The Flatpak machine

On a PC, Flatpak apps run in a machine of their own, `flatpak`, labelled
purple, and nothing of Flatpak is on the host. `nixos/modules/flatpak-machine.nix`
adds its template, `losos-flatpak` (`nixos/guest/flatpak.nix`), and has the
host make the machine at its first boot (`losos.machines.system`, above).
The template is the LosOS guest's machine with Flatpak and Flathub, the
same `flatpak.nix` a phone runs on its host ([Halium GSI](halium.md)); its
user may install Flatpaks for the whole machine without being asked. It
has no store of its own: the host's [Store](store.md) installs into it. A Flatpak's own
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
  At the first boot with the machine, `losos-flatpak-move` moves
  `/var/lib/flatpak` (the apps installed for everyone) in and deletes it;
  at each administrator's next sign-in, a user service moves their
  `~/.local/share/flatpak` (apps installed for themselves) in and deletes
  it, and copies `~/.var/app` (every app's data) to the machine's user,
  leaving the host's copy for its owner to delete. Each move,
  `losos-flatpak-move-host`, makes the files an ext4 disk in libvirt's
  `default` pool, gives it to the machine read-only for one boot with the
  serial `losos-flatpak-move`, has the guest's `losos-flatpak-move` copy it
  where it belongs, shuts the machine down and deletes the disk. The
  machine must be off for it, since the disk is added to its definition:
  a move that finds it running waits for the next boot or sign-in.

## Machines from an installer CD

A machine can also start empty and install a system from its own CD, as a
PC would:

```sh
pm machine create win --installer Win11_24H2_English_x64.iso --system windows --label red
pm machine create fedora --installer Fedora-Workstation-Live-43.iso --label green
```

Each CD becomes a read-only volume in the `default` pool, shared by every
machine given the same bytes, and the machine's disk starts empty. It boots
its disk first and the first CD while the disk has nothing on it, so the
installer runs once and the installed system starts after it. Such a
machine has a screen (SPICE, attached through libvirt and never on a port,
with no clipboard channel), a USB tablet, and its CDs on SATA, which every
installer has a driver for. The desktop shows it in a window of its own
(see "On the desktop"); outside a session,
`virt-viewer --attach -c qemu:///system win` does.

pm also gives it the image's **CD of guest tools** for its system, which
the image ships at `share/pm/machines/<system>-tools.iso` where pm looks
for it (`nixos/modules/machines.nix`, sources in `nixos/guest-tools/`).

### Windows

A Windows machine is what Windows 11 asks of a PC: UEFI, a TPM 2.0 (swtpm,
its state kept and deleted with the machine), Hyper-V's enlightenments when
the host has KVM, its clock in local time, 4096 MiB and 64 GiB unless told
otherwise. It has no vsock, which Windows has no use for. x86-64 only.

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
account. The firmware's "Press any key to boot from CD" is the person's to
answer on the machine's screen; missed, the machine stops at the
firmware's boot menu, and starting it again offers it again.

**WinApps** ([winapps-org/winapps](https://github.com/winapps-org/winapps),
`nixos/pkgs/winapps.nix`) then opens the machine's Windows apps as windows
of their own on the desktop, each a FreeRDP RemoteApp session. It is set up
as its own documentation says for libvirt, in
`~/.config/winapps/winapps.conf`: `WAFLAVOR="libvirt"`, `VM_NAME` the
machine's name, and `RDP_USER` and `RDP_PASS` the Windows account; then
`winapps-setup --user` lists the machine's apps into the app grid. Remote
Desktop needs Windows' Pro edition or above.

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

Under libvirt without KVM, in the machines VM test
(`nixos/tests/machines.nix`), a Windows machine from a stand-in CD is
defined with the drivers CD, a TPM and a screen, starts with its swtpm, and
is removed leaving both CDs for other machines. No Windows or other
distribution has been installed from them yet.
