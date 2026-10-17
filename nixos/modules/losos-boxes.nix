# LosOS boxes, the home servers of the LosOS project, from this desktop
# (docs/losos-boxes.md): `losos`, the boxes' own terminal client, which
# manages one with the panes of its admin pages and needs no SSH, and
# losos-boxes, which finds the boxes on the local network and offers each
# one in the palette and the app list.
#
# On a PC both run in the LosOS boxes machine (nixos/guest/boxes.nix),
# which this module adds to machines.nix's templates, and not on the host: the
# machine is the one place that talks to boxes, and the host only lends it
# its resolved, which does the mDNS a machine behind the host's NAT cannot.
# A phone has no machines (the GSI does not import quickemu.nix), so there
# the session runs them itself.
{
  config,
  options,
  lib,
  pkgs,
  ...
}:

let
  # quickemu.nix declares the machines' network; a system without it has
  # no machines.
  machines = lib.hasAttrByPath [ "losos" "machines" "network" ] options;
in
{
  config = lib.mkMerge [
    (lib.mkIf (!machines) {
      environment.systemPackages = [
        pkgs.losos
        # `losos-boxes list` prints what the service sees, and the entries'
        # terminal action runs `losos-boxes terminal`.
        pkgs.losos-boxes
      ];

      # One per session, since the entries are the signed-in person's: it
      # browses through resolved, which answers mDNS on this OS (network.nix),
      # so no Avahi. The entries go when the session does, so a box that left
      # while nobody watched is not offered at the next sign-in.
      systemd.user.services.losos-boxes = {
        description = "Offer the LosOS boxes on this network as applications";
        # The login screen's own user, and any other system account that gets
        # a user manager, is not a person.
        unitConfig.ConditionUser = "!@system";
        wantedBy = [ "graphical-session.target" ];
        partOf = [ "graphical-session.target" ];
        after = [ "graphical-session.target" ];
        serviceConfig = {
          ExecStart = "${lib.getExe pkgs.losos-boxes} watch";
          ExecStopPost = "${lib.getExe pkgs.losos-boxes} clear";
          # resolved restarting ends a browse; the next run starts over.
          Restart = "on-failure";
          RestartSec = "10s";
        };
      };
    })

    # The template, wherever machines.nix builds templates. It is looked
    # for among the options rather than behind `machines`, since a system
    # without machines.nix has no such option to set.
    (lib.optionalAttrs (options.losos ? machines) {
      losos.machines.guests.losos-boxes = ../guest/boxes.nix;
    })

    # optionalAttrs rather than mkIf, as for the template: a system without
    # quickemu.nix has no losos.machines.network to set.
    (lib.optionalAttrs machines {
      # The LosOS boxes machine's way to this host's resolved, and to the
      # other machines (`losos-boxes relay`), for a running machine pm
      # recorded as made from the losos-boxes template; any other machine is
      # refused. On vsock port 7351 it is let through to resolved's Varlink
      # socket: lookups only, which change nothing on the host. On port 7352
      # it names the boxes it found, and the relay writes those names at its
      # address into a hosts file of HOSTS_DIR, below, so the other machines'
      # connections to a box go to it.
      systemd.services.losos-boxes-relay = {
        description = "Lend the LosOS boxes machine this host's resolved";
        wantedBy = [ "multi-user.target" ];
        after = [ "systemd-resolved.service" ];
        serviceConfig = {
          ExecStart = "${lib.getExe pkgs.losos-boxes} relay";
          # A user of its own rather than a dynamic one, as it owns the hosts
          # files, which outlive a restart. resolved's Varlink socket is open
          # to every account, and a vsock port above 1023 needs nothing.
          User = "losos-boxes";
          Group = "losos-boxes";
          # A machine is its owner's: its record (machine.json) is in their
          # home, and its address in the lease file of the machines' DHCP,
          # which is its dynamic user's. Reading both takes reading any file;
          # writing stays limited to its own directory.
          AmbientCapabilities = [ "CAP_DAC_READ_SEARCH" ];
          CapabilityBoundingSet = [ "CAP_DAC_READ_SEARCH" ];
          Restart = "on-failure";
          ProtectSystem = "strict";
          ReadWritePaths = [ "/run/losos-boxes" ];
          ProtectHome = "read-only";
          PrivateTmp = true;
          RestrictAddressFamilies = [
            "AF_UNIX"
            "AF_VSOCK"
          ];
        };
      };
      users.users.losos-boxes = {
        isSystemUser = true;
        group = "losos-boxes";
      };
      users.groups.losos-boxes = { };
      # Made at boot, before the machines' dnsmasq starts and watches it, and
      # kept while the relay restarts.
      systemd.tmpfiles.rules = [
        "d /run/losos-boxes 0755 losos-boxes losos-boxes -"
        "d /run/losos-boxes/hosts 0755 losos-boxes losos-boxes -"
      ];

      # The machines' dnsmasq reads every hosts file there, and each again
      # whenever it changes, so a box's name answered with the LosOS boxes
      # machine's address is answered before dnsmasq asks the host's resolved
      # for it (docs/hypervisor.md, "Names on the network").
      #
      # libvirt's dnsmasq took it as an option of its network's definition,
      # which a service of this module's added once and restarted the network
      # for; the machines' dnsmasq is the OS's own now, and takes it outright.
      losos.machines.network.hostsDirs = [ "/run/losos-boxes/hosts" ];
    })
  ];
}
