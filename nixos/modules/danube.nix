# The web browser: Danube, WPE WebKit drawn inside an mcsapi window
# (nixos/pkgs/danube.nix, docs/danube.md). It is what opens a link or an
# HTML file unless the user picks something else, and the window that
# opens when a network wants a sign-in first (a captive portal).
#
# Uranium, Chromium under the OS's name with a Flatpak build, was here
# before; Danube replaced it, and its options went with it
# (docs/danube.md, "What it replaced").
{
  config,
  lib,
  pkgs,
  ...
}:

let
  portal = config.losos.captivePortal;
in
{
  # losos.danube.home and losos.danube.jit wrote /etc/danube/settings.conf
  # under the user's own; nothing set them, so they went, and Danube's
  # built-in defaults (an empty page, the JIT off) are what a user who
  # writes no ~/.config/danube/settings.conf gets (docs/danube.md,
  # "Settings").
  options.losos.captivePortal = {
    enable = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = ''
        Check for a captive portal whenever the network changes, and open
        Danube's sign-in window when one is in the way. The check is
        GNOME's, as NetworkManager's GNOME setup uses (captive.rs); the
        `checkUrl` and `expect` options that chose another went, since
        nothing set them.
      '';
    };
  };

  config = {
    environment.systemPackages = [ pkgs.danube ];

    # One per session: it opens a window, so it runs as the person signed
    # in, and only while their desktop does.
    systemd.user.services.danube-captive-watch = lib.mkIf portal.enable {
      description = "Open Danube's sign-in window for a captive portal";
      wantedBy = [ "graphical-session.target" ];
      partOf = [ "graphical-session.target" ];
      after = [ "graphical-session.target" ];
      # nmcli, which it watches NetworkManager with (captive.rs).
      path = [ config.networking.networkmanager.package ];
      serviceConfig = {
        ExecStart = "${lib.getExe pkgs.danube} captive-watch";
        Restart = "on-failure";
      };
    };

    # The default for web pages and links, which xdg-open, the portal's
    # OpenURI and derisk's own launcher all read from mimeapps.list.
    xdg.mime.defaultApplications = lib.genAttrs [
      "text/html"
      "application/xhtml+xml"
      "x-scheme-handler/http"
      "x-scheme-handler/https"
    ] (_: "org.losos.Danube.desktop");
  };
}
