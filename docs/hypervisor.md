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
the host.

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

