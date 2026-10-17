# Flatpak on a PC: a machine of its own (docs/hypervisor.md, "The Flatpak
# machine"), and nothing of Flatpak on the host. Flatpak and Flathub run
# in the machine made from nixos/guest/flatpak.nix, which this module adds
# to the templates as losos-flatpak and has made for every person as their
# machine `flatpak`, labelled purple. Its apps are on the desktop as that
# machine's, in the palette whether it runs or not; picking one starts it.
# The host's Store installs them there, and Settings' Privacy page changes
# their permissions there.
#
# Flatpak ran on the host before, so a host that updates into this may
# have apps and their data where the host kept them. They move into each
# person's Flatpak machine once, at their next sign-in, when their home is
# open: the apps installed for everyone, their own apps, and every app's
# data.
#
# A phone has no machines; the GSI imports flatpak.nix itself and keeps
# Flatpak on the host.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  machine = "flatpak";

  moveHost = pkgs.writeShellApplication {
    name = "losos-flatpak-move-host";
    runtimeInputs = [
      pkgs.quickemu
      pkgs.socat
      config.systemd.package
      pkgs.coreutils
      pkgs.findutils
      pkgs.e2fsprogs
      pkgs.jq
    ];
    text = builtins.readFile ./flatpak-move-host.sh;
  };
in
{
  losos.machines.guests.losos-flatpak = ../guest/flatpak.nix;

  losos.machines.system.${machine} = {
    template = "losos-flatpak";
    label = "purple";
  };

  # What Flatpak kept on the host: the apps it installed for everyone
  # (/var/lib/flatpak), the person's own (~/.local/share/flatpak) and every
  # app's data (~/.var/app), each moved once into the person's own Flatpak
  # machine, at sign-in, when a homed home is open and the machine is made
  # (machines.nix). Their own apps go from the host once moved; their data
  # stays until its owner deletes it, and a mark says each was moved. A
  # machine that is running is not moved into; the next sign-in tries again.
  #
  # A system service moved /var/lib/flatpak once, into the one machine
  # libvirt kept for everyone, and removed it, and only administrators'
  # own files moved, since polkit asked anyone else for a password to
  # manage a machine. Every person has their own machine now and manages
  # it without asking, so everyone's move here, and /var/lib/flatpak stays
  # for whoever has not signed in since (docs/not-done.md).
  systemd.user.services.losos-flatpak-move = {
    description = "Move your Flatpak apps and their data into your Flatpak machine";
    unitConfig.ConditionUser = "!@system";
    wantedBy = [ "default.target" ];
    wants = [ "losos-system-machines.service" ];
    after = [ "losos-system-machines.service" ];
    path = [ pkgs.coreutils ];
    serviceConfig = {
      Type = "oneshot";
      SuccessExitStatus = [ 75 ];
    };
    script = ''
      state="''${XDG_STATE_HOME:-$HOME/.local/state}/losos"
      folder="''${XDG_DATA_HOME:-$HOME/.local/share}/pm/machines/${machine}"
      if [ ! -e "$folder/machine.json" ]; then
        exit 0
      fi
      mkdir -p "$state"
      # move <kind> <dir> <what of it> [remove]: once per kind, when there
      # is something to move, and the person's own copy removed after.
      move() {
        if [ -e "$state/flatpak-moved-$1" ]; then
          return 0
        fi
        if [ -n "$(ls -A "$2/$3" 2>/dev/null)" ]; then
          ${lib.getExe moveHost} ${machine} "$1" "$2"
          if [ "''${4:-}" = remove ]; then
            rm -rf "$2"
          fi
        fi
        touch "$state/flatpak-moved-$1"
      }
      move system /var/lib/flatpak app
      move user-flatpak "$HOME/.local/share/flatpak" app remove
      move var-app "$HOME/.var/app" .
    '';
  };
}
