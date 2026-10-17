# The LosOS guest: the machine template for a person's own apps
# (docs/hypervisor.md, "The guest template"). common.nix is the machine;
# this adds what a person works with in it: Flatpak with Flathub and Bazaar,
# as on the host, so apps installed in a machine come from where the host's
# come from, and derisk's Calculator.
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
  imports = [
    ./common.nix
    ../modules/flatpak.nix
  ];

  # flatpak's own rule lets an active, local wheel member install for the
  # whole system. The guest's user is never in an active session (nobody
  # logs in; its apps run under its systemd manager), so it is given
  # Flatpak's actions here instead. The machine is its owner's to fill.
  security.polkit.extraConfig = ''
    polkit.addRule(function (action, subject) {
      if (action.id.indexOf("org.freedesktop.Flatpak.") === 0 && subject.user === "user") {
        return polkit.Result.YES;
      }
      return polkit.Result.NOT_HANDLED;
    });
  '';

  environment.systemPackages = [ calculator ];
}
