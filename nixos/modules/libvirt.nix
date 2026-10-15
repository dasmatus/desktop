# The PC is a hypervisor: Linux with KVM runs the machines, and libvirt is
# the one place they are defined, started and stopped (docs/hypervisor.md).
# derisk's desktop shows each machine's windows framed in its label colour,
# its Machines app makes and runs them, pm (`pm machine`, through its libvirt
# plugin) installs them, and virt-manager and virsh see the same machines,
# because all of them talk to the same daemon at qemu:///system. Nothing
# here starts QEMU on its own or keeps its own list of machines.
#
# All of it is state under /var/lib/libvirt and /run/libvirt on the root
# partition, so the verity /usr and the update model are untouched, and nothing
# in it is per machine: a PC without hardware virtualisation gets the same
# image, and libvirt reports that its QEMU emulates rather than runs on KVM.
#
# The GSI does not import this (nixos/halium/): most phones' kernels have no
# KVM, so apps there keep running on the host.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  libvirt = config.virtualisation.libvirtd.package;
in
{
  virtualisation.libvirtd = {
    enable = true;
    # QEMU for the host's own architecture. The default, pkgs.qemu, emulates
    # every target QEMU has, which is most of its size and nothing a desktop's
    # machines run.
    qemu.package = pkgs.qemu_kvm;
    # The TPM a Windows machine is given (pm's libvirt plugin), which
    # Windows 11 will not install without: libvirt runs an swtpm for each
    # such machine and keeps its state beside the machine's.
    qemu.swtpm.enable = true;
  };

  # libvirtd starts at boot, as NixOS has it, rather than only on its socket
  # as this module once made it: it is the control plane now, not a tool
  # someone occasionally opens, and the machines and the network marked to
  # start with the host need it running to start. It still exits two minutes
  # after its last client and machine are gone (the module's --timeout 120),
  # which on a desktop never happens while a session watches the machines.
  #
  # This is libvirt's monolithic daemon, not its modular virtqemud and friends:
  # the NixOS module wires only libvirtd (its config, the qemu.conf it copies to
  # /var/lib/libvirt, the stable emulator links), and taking the modular daemons
  # would mean re-doing that by hand for the same API on the same socket path.

  # Every program that connects without naming a URI (virsh, pm and its libvirt
  # plugin, virt-manager's first connection) reaches the system daemon, where
  # the machines are, rather than a per-user qemu:///session that would have
  # none of them. A session daemon could not give a machine the vsock its
  # windows come through anyway: /dev/vhost-vsock is root's.
  environment.sessionVariables.LIBVIRT_DEFAULT_URI = "qemu:///system";

  # virtio-vsock's host side: the device libvirt gives each machine's
  # <vsock>, and the transport derisk's gate listens on for the guests'
  # windows. It loads on demand when QEMU opens /dev/vhost-vsock, but the gate
  # binds its port when the session starts, before any machine has.
  boot.kernelModules = [ "vhost_vsock" ];

  # The other end of the guests' windows: derisk starts `waypipe client` for
  # each running machine, by name from the session's PATH, and it turns what
  # the guest's `waypipe server` sends back into Wayland.
  #
  # virt-viewer shows a machine's own screen, which a machine that installs
  # from a CD has for its installer and a Windows machine for its whole
  # desktop; derisk starts it attached through libvirt, on the machine's
  # labelled socket. WinApps opens a Windows machine's apps as windows of
  # their own instead, over RDP through FreeRDP, which it brings; Windows
  # machines are x86-64 only (pm's libvirt plugin).
  environment.systemPackages = [
    pkgs.waypipe
    pkgs.virt-viewer
  ]
  ++ lib.optional pkgs.stdenv.hostPlatform.isx86_64 pkgs.winapps;

  # Who may manage machines on qemu:///system. NixOS grants it to the libvirtd
  # group, but people here are systemd-homed records the module system never
  # sees (accounts.nix), so nobody is in that group. Administrators get it
  # instead, the way Ubuntu and Fedora put their installer's admin user in
  # libvirt's group: managing system machines can hand a guest any host disk,
  # so it is an administrator's power, and it is not asked for again on every
  # connection because the desktop, virt-manager and pm connect many times a
  # session. Everyone else is asked for an administrator's password by
  # polkit's agent. Watching (a read-only connection: which machines exist
  # and run, their labels, which vsock address is whose) needs no grant, so
  # every session frames its machines' windows either way.
  security.polkit.extraConfig = ''
    polkit.addRule(function (action, subject) {
      if (action.id === "org.libvirt.unix.manage" && subject.isInGroup("wheel")) {
        return polkit.Result.YES;
      }
      return polkit.Result.NOT_HANDLED;
    });
  '';

  # The `default` storage pool and the `default` network are where pm's
  # libvirt plugin puts every machine's disk and network card, as virt-manager
  # does when told nothing else. NixOS copies libvirt's own definition of the
  # network into /var/lib/libvirt but leaves it stopped, and defines no pool
  # at all; virt-manager offers to make both the first time it is opened. Here
  # they exist and start with the host before anyone asks, through libvirt's
  # own API, so a definition someone changed in virt-manager stays theirs.
  systemd.services.libvirt-defaults = {
    description = "Define and start libvirt's default storage pool and network";
    wantedBy = [ "multi-user.target" ];
    requires = [ "libvirtd.service" ];
    after = [ "libvirtd.service" ];
    # Before machines that were running at shutdown come back, so their
    # disks and network are there.
    before = [ "libvirt-guests.service" ];
    path = [ libvirt ];
    environment.LIBVIRT_DEFAULT_URI = "qemu:///system";
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    script = ''
      if ! virsh -q pool-info default >/dev/null 2>&1; then
        virsh -q pool-define-as default dir --target /var/lib/libvirt/images
        virsh -q pool-build default
      fi
      virsh -q pool-autostart default
      if [ "$(virsh -q pool-info default | sed -n 's/^State: *//p')" != running ]; then
        virsh -q pool-start default
      fi

      virsh -q net-autostart default
      if [ "$(virsh -q net-info default | sed -n 's/^Active: *//p')" != yes ]; then
        virsh -q net-start default
      fi
    '';
  };

  # libvirt's network hands its machines addresses and names with dnsmasq on
  # virbr0, and NixOS's firewall drops whatever it was not told to accept,
  # which libvirt's own nftables table cannot overrule: a packet another
  # table drops stays dropped. So the guests could not get an address. DHCP
  # and DNS on that bridge only; forwarding their traffic out is libvirt's
  # own masquerading, which the firewall leaves alone (it filters input).
  networking.firewall.interfaces.virbr0 = {
    allowedUDPPorts = [
      53
      67
    ];
    allowedTCPPorts = [ 53 ];
  };
}
