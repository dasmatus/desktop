# pm stays the system manager. This image comes from nixpkgs, but software a
# user builds or runs on the machine goes through pm: signed recipes built into
# .cpkg archives, their entrypoints run in pm's sandbox, and pmd as its worker.
#
# Nothing extra is needed for that sandbox. It is unprivileged user namespaces
# plus landlock, both of which NixOS's kernel allows by default.
#
# pm also administers the machine, as a user of its own. `pm system` asks
# `pmd --system`, which runs as `pm` and asks polkit about each caller, and an
# administrator authenticates as themselves before pm starts, stops or
# restarts a unit, updates the system, or runs a command as root. That is what
# sudo, su and run0 were for, and none of them is in the image any more
# (accounts.nix). docs/pm.md has the commands.
{ lib, pkgs, ... }:

#
# pm's plugins are shipped under /run/current-system/sw/share/pm/plugins and
# not installed into anyone's config: pm runs a plugin only when it is signed
# by a key its user trusts, and that trust is the user's to give. Where a user
# runs Home Manager, the flake's homeModules.pm (nixos/home/pm.nix) installs
# and signs them from their own configuration.
{
  environment.systemPackages = [
    pkgs.pm
    pkgs.pm-plugins
  ];
  environment.pathsToLink = [ "/share/pm" ];

  # The identity the system trusts to manage units. A system user with no
  # shell and no password: nobody logs in as pm, only the daemon runs as it.
  users.users.pm = {
    isSystemUser = true;
    group = "pm";
    description = "pm, the system manager";
    home = "/var/lib/pm";
  };
  users.groups.pm = { };

  # The D-Bus policy (only pm owns org.pm1.System) and the activation file,
  # from pm's data/; its polkit actions arrive through the system profile.
  services.dbus.packages = [ pkgs.pm ];

  # Started by the bus on the first call and gone after a minute idle, so it
  # costs nothing on a machine nobody administers.
  systemd.services.pm-system = {
    description = "pm's system service";
    documentation = [ "https://github.com/losos-project/pm#administering-the-machine" ];
    # The PATH a command run as root resolves in and inherits, the one a
    # login shell has.
    path = [ "/run/current-system/sw" ];
    serviceConfig = {
      Type = "dbus";
      BusName = "org.pm1.System";
      ExecStart = "${pkgs.pm}/bin/pmd --system";
      User = "pm";
      Group = "pm";
      StateDirectory = "pm";
      # Everything the daemon does, it does over the system bus, as pm: it
      # needs no capability, no network, and nothing of the file system but
      # the pseudo-terminals it checks the owner of and the programs it
      # looks up on PATH.
      CapabilityBoundingSet = "";
      NoNewPrivileges = true;
      ProtectSystem = "strict";
      ProtectHome = true;
      PrivateTmp = true;
      PrivateNetwork = true;
      ProtectKernelTunables = true;
      ProtectKernelModules = true;
      ProtectKernelLogs = true;
      ProtectControlGroups = true;
      ProtectClock = true;
      ProtectHostname = true;
      RestrictAddressFamilies = [ "AF_UNIX" ];
      RestrictNamespaces = true;
      RestrictRealtime = true;
      RestrictSUIDSGID = true;
      LockPersonality = true;
      MemoryDenyWriteExecute = true;
      SystemCallArchitectures = "native";
      SystemCallFilter = [ "@system-service" ];
      UMask = "0077";
    };
  };

  security.polkit.extraConfig = lib.mkMerge [
    ''
      // pm manages units without asking anyone: pmd --system has already
      // asked polkit about its caller (org.pm1.system.*) before it does.
      polkit.addRule(function (action, subject) {
        if (action.id === "org.freedesktop.systemd1.manage-units" && subject.user === "pm") {
          return polkit.Result.YES;
        }
        return polkit.Result.NOT_HANDLED;
      });
    ''
    # Last, so a narrower rule before it (disk.nix lets an administrator ask
    # for a factory reset) still answers first. Without it an administrator
    # could still drive systemd directly, and run0 or `systemd-run` would hand
    # them root past pm; with it, `systemctl restart` and run0 are refused and
    # pm is the one way in. Root is never asked: systemd lets it through
    # without polkit.
    (lib.mkAfter ''
      polkit.addRule(function (action, subject) {
        var systemd = [
          "org.freedesktop.systemd1.manage-units",
          "org.freedesktop.systemd1.manage-unit-files",
          "org.freedesktop.systemd1.reload-daemon",
          "org.freedesktop.systemd1.set-environment",
        ];
        if (systemd.indexOf(action.id) >= 0 && subject.user !== "pm") {
          return polkit.Result.NO;
        }
        return polkit.Result.NOT_HANDLED;
      });
    '')
  ];
}
