# The LosOS guest: the machine template for a person's own apps
# (docs/hypervisor.md, "The guest template"). common.nix is the machine;
# this adds derisk's Calculator. Flatpak is not here: Flatpak apps have a
# machine of their own, flatpak.nix, so there is one place they are
# installed, updated and given permissions.
{ pkgs, ... }:

let
  # derisk's Calculator, the one core app that runs as a window of its own
  # (derisk-gpui); the rest are drawn by the compositor, which a guest does
  # not have. Its desktop file is not in the package, which installs none:
  # on the host the compositor launches it.
  calculator = pkgs.makeDesktopItem {
    name = "org.derisk.calculator";
    desktopName = "Calculator";
    exec = "${pkgs.derisk.gpui}/bin/derisk-gpui org.derisk.calculator";
    icon = "accessories-calculator";
    categories = [
      "Utility"
      "Calculator"
    ];
  };
in
{
  imports = [ ./common.nix ];

  environment.systemPackages = [ calculator ];
}
