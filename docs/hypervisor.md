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
  and, while it runs, "Terminal in work" for each app it has and "Shut down
  work". A machine's label colour is among its search words, so typing
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

The image ships one template, the LosOS guest:
`share/losos/machines/losos-guest.qcow2` on `XDG_DATA_DIRS`, in `/usr` and
covered by its verity hash like the rest of the OS. `nixos/guest/` is its
NixOS configuration, and `nixos/modules/machines.nix` evaluates it with the
image's own package set and turns it into the template. An update brings a
new template; machines made from the old one keep it.

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
  boot (lingering). Nobody logs in: its apps run under that manager;
- **its home, from Home Manager** (`nixos/guest/home.nix`, added by the
  flake): activated at every boot before that manager starts, it makes the
  folders apps expect to find in a home (`nixos/modules/home-folders.nix`:
  the XDG base directories, fonts, icons and derisk's), which the OS's own
  users get from per-user tmpfiles instead, as their names are not known
  when the image is built. Home Manager's activation needs a working Nix,
  so the guest has the Nix daemon, which the user may use too;
- **a waypipe server** in that manager, `waypipe --vsock --socket 2:7350
  --no-gpu --display wayland-0 server`. Every app the user runs finds it as
  `WAYLAND_DISPLAY=wayland-0`, and each app's connection becomes a vsock
  stream to the host's desktop, port 7350. There is no GPU in a machine, so
  apps draw with Mesa's llvmpipe and lavapipe into shared memory, which is
  what waypipe carries;
- **apps**: derisk's Terminal (`derisk-terminal`, drawing in software, as
  there is no GPU), Danube, derisk's Calculator (the one derisk core app that
  is a window of its own, `derisk-gpui`; the rest are drawn by the host's
  compositor), and Flatpak with Flathub and Bazaar, as on the host. The
  guest's user may install Flatpaks for the whole machine without being
  asked;
- **networkd**, taking an address from libvirt's `default` network.

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
