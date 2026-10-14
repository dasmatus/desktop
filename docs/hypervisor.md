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
the host and the machine template it ships.

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
- **a waypipe server** in that manager, `waypipe --vsock --socket 2:7350
  --no-gpu --display wayland-0 server`. Every app the user runs finds it as
  `WAYLAND_DISPLAY=wayland-0`, and each app's connection becomes a vsock
  stream to the host's desktop, port 7350. There is no GPU in a machine, so
  apps draw with Mesa's llvmpipe and lavapipe into shared memory, which is
  what waypipe carries;
- **apps**: foot, Danube, derisk's Calculator (the one derisk core app that
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
