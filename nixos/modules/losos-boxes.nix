# LosOS boxes, the home servers of the LosOS project, from this desktop
# (docs/losos-boxes.md): `losos`, the boxes' own terminal client, which
# manages one with the panes of its admin pages and needs no SSH, and
# losos-boxes, which finds the boxes on the local network and offers each
# one in the palette and the app list.
{ lib, pkgs, ... }:

{
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
}
