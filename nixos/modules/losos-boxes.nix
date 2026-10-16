# LosOS boxes, the home servers of the LosOS project, from this desktop
# (docs/losos-boxes.md): `losos`, the boxes' own terminal client, which
# manages one with the panes of its admin pages and needs no SSH.
{ pkgs, ... }:

{
  environment.systemPackages = [ pkgs.losos ];
}
