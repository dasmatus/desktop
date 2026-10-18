# The PC is a hypervisor: Linux with KVM runs the machines, and quickemu
# starts each one (docs/hypervisor.md). A machine is a quickemu folder: its
# <name>.conf, its disk, its firmware's variables and its TPM's state, and
# pm's own record of it, machine.json. pm (`pm machine`, through its
# quickemu plugin) makes and removes them, a systemd unit per machine runs
# `quickemu --vm`, and derisk's desktop shows each machine's windows framed
# in its label colour. There is no daemon between them: quickemu starts QEMU
# and leaves, and what a machine is lives in its folder.
#
# This module is what the machines share on the host: the network they are
# on, QEMU's bridge helper that puts them there, the vsock their windows come
# through, and quickemu itself. Nothing in it is per machine, and a PC
# without hardware virtualisation gets the same image: quickemu falls back
# to emulating, slowly.
#
# libvirt did this until quickemu took its place. Its daemon was a second
# record of every machine beside the files that are the machine, and the
# reason a machine could only be managed through it; quickemu needs neither.
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
  cfg = config.losos.machines.network;

in
{
  options.losos.machines.network = {
    hostsDirs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = ''
        Folders of hosts files the machines' DNS answers from as well as the
        host's resolver, re-read whenever a file in them changes. The LosOS
        boxes machine puts the boxes' names in one (losos-boxes.nix).
      '';
    };
  };

  config = {
    environment.systemPackages = [
      # With TPM-capable firmware and the bridge helper NixOS made setuid
      # (nixos/pkgs/default.nix).
      pkgs.quickemu
      # The other end of the guests' windows: derisk starts `waypipe client`
      # for each running machine, by name from the session's PATH, and it
      # turns what the guest's `waypipe server` sends back into Wayland.
      pkgs.waypipe
      # remote-viewer shows a machine's own screen, which a machine that
      # installs from a CD has for its installer and a Windows machine for
      # its whole desktop: quickemu's SPICE on the Unix socket in the
      # machine's folder.
      pkgs.virt-viewer
    ];

    # virtio-vsock's host side: the device each machine gets through its
    # .conf's extra_args, and the transport derisk's gate listens on for the
    # guests' windows. It loads on demand when QEMU opens /dev/vhost-vsock,
    # but the gate binds its port when the session starts, before any
    # machine has. udev's default rule leaves /dev/vhost-vsock, like
    # /dev/kvm, open to everyone.
    boot.kernelModules = [ "vhost_vsock" ];

    # The machines' network, `virbr0`, 192.168.122.0/24: the bridge, address
    # and NAT libvirt used to make, now networkd's, so a machine's NetworkManager
    # profile, its `.local` lookups through the host and the LosOS boxes'
    # routing all see the network they always did. A bridge with no machine
    # on it has no carrier, and its address must be there before the first
    # one starts.
    systemd.network = {
      netdevs."40-virbr0".netdevConfig = {
        Kind = "bridge";
        Name = "virbr0";
      };
      networks."40-virbr0" = {
        matchConfig.Name = "virbr0";
        address = [ "192.168.122.1/24" ];
        networkConfig = {
          ConfigureWithoutCarrier = true;
          # Forward and masquerade what the machines send out, as libvirt's
          # NAT did. networkd adds the nftables rule itself.
          IPMasquerade = "ipv4";
          LinkLocalAddressing = "no";
          IPv6AcceptRA = false;
          # resolved asks the machines' link for `.local` names too, for
          # machines installed from a CD, which multicast for themselves.
          MulticastDNS = true;
          # And the machines' dnsmasq for `.machines` names, the names
          # machines gave its DHCP, and nothing else, so the host reaches a
          # machine by the name it calls itself (docs/hypervisor.md, "Names
          # on the network").
          DNS = [ "192.168.122.1" ];
          Domains = [ "~machines" ];
          DNSDefaultRoute = false;
        };
        linkConfig.RequiredForOnline = "no";
      };
    };

    # Addresses and DNS for the machines, which libvirt's dnsmasq used to
    # give. DNS goes to the host's own resolver, resolved's stub, so a
    # machine asks the host for `.local` names (docs/hypervisor.md, "Names on
    # the network"); hostsDirs answers before it.
    systemd.services.losos-machines-dns = {
      description = "DHCP and DNS for the machines on virbr0";
      wantedBy = [ "multi-user.target" ];
      bindsTo = [ "sys-subsystem-net-devices-virbr0.device" ];
      after = [
        "sys-subsystem-net-devices-virbr0.device"
        "systemd-networkd.service"
        "systemd-resolved.service"
      ];
      serviceConfig = {
        ExecStart = lib.escapeShellArgs (
          [
            "${pkgs.dnsmasq}/bin/dnsmasq"
            "--keep-in-foreground"
            "--conf-file=/dev/null"
            "--interface=virbr0"
            "--bind-dynamic"
            "--except-interface=lo"
            "--dhcp-range=192.168.122.2,192.168.122.254,1h"
            "--dhcp-authoritative"
            # A machine's own name for itself, from its DHCP request, as
            # <name>.machines, which it answers and never passes on, so a
            # question resolved routes here cannot come back to it.
            "--domain=machines"
            "--local=/machines/"
            "--dhcp-leasefile=/var/lib/losos-machines-dns/leases"
            "--no-resolv"
            "--no-hosts"
            "--server=127.0.0.53"
          ]
          ++ map (dir: "--hostsdir=${dir}") cfg.hostsDirs
        );
        DynamicUser = true;
        StateDirectory = "losos-machines-dns";
        # Ports 53 and 67 and DHCP's raw socket, as an unprivileged user.
        AmbientCapabilities = [
          "CAP_NET_BIND_SERVICE"
          "CAP_NET_ADMIN"
          "CAP_NET_RAW"
        ];
        CapabilityBoundingSet = [
          "CAP_NET_BIND_SERVICE"
          "CAP_NET_ADMIN"
          "CAP_NET_RAW"
        ];
        Restart = "on-failure";
      };
    };

    # NixOS's firewall drops what it was not told to accept: DHCP and DNS
    # from the machines on virbr0, nothing else. Forwarding their traffic out
    # is networkd's masquerading, which the firewall leaves alone (it filters
    # input).
    networking.firewall.interfaces.virbr0 = {
      allowedUDPPorts = [
        53
        67
      ];
      allowedTCPPorts = [ 53 ];
    };

    # QEMU's bridge helper makes a machine's tap device and adds it to
    # virbr0, which takes CAP_NET_ADMIN, so it is setuid as NixOS made it for
    # libvirt; quickemu runs it as the machine's owner. It adds taps to the
    # bridges bridge.conf allows and no other.
    security.wrappers.qemu-bridge-helper = {
      setuid = true;
      owner = "root";
      group = "root";
      # From the QEMU quickemu runs, which nixpkgs builds with Samba for its
      # shared folder: plain pkgs.qemu would be a second QEMU in the image.
      source = "${pkgs.qemu.override { smbdSupport = true; }}/libexec/qemu-bridge-helper";
    };
    environment.etc."qemu/bridge.conf".text = ''
      allow virbr0
    '';
  };
}
