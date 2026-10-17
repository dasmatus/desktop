# Backups to a LosOS box (docs/backup.md): each person's Git repositories to
# the box's LosOS Git, and everything else in their home to their LosOS
# cloud, from a timer of their own. derisk Settings' Backups page chooses
# the box, signs in to both and turns it on, all through `losos-backup`.
#
# It runs in the person's session because their home is theirs alone and
# systemd-homed unlocks it only while they are signed in. On a PC it reaches
# the box through the LosOS boxes machine (`losos-boxes route`), the one
# machine that talks to boxes, starting it for the run when it is stopped;
# on a phone, which has no machines, it reaches the box by name.
{
  options,
  lib,
  pkgs,
  ...
}:

let
  # quickemu.nix declares the machines' network; a system without it has
  # no machines.
  machines = lib.hasAttrByPath [ "losos" "machines" "network" ] options;
  unit = {
    # The login screen's own user, and any other system account that gets
    # a user manager, has no home to back up.
    unitConfig.ConditionUser = "!@system";
    # git does the pushing. pm starts the person's LosOS boxes machine, and
    # systemd-run, which it starts it through, looks quickemu up here.
    path = [
      pkgs.git
      pkgs.pm
    ]
    ++ lib.optional machines pkgs.quickemu;
    serviceConfig = {
      Type = "oneshot";
      # A backup is never in a hurry: the session comes first.
      Nice = 10;
      IOSchedulingClass = "idle";
      CPUSchedulingPolicy = "batch";
    };
  };
in
{
  environment.systemPackages = [ pkgs.losos-backup ];

  # The timer's: it backs up only when the backups are on and the last one
  # that went through is a day old, so it is cheap to ask every hour, and a
  # computer that was off or away from the box catches up within the hour
  # after it is back.
  systemd.user.services.losos-backup = lib.recursiveUpdate unit {
    description = "Back up to the LosOS box when a day has passed";
    serviceConfig.ExecStart = "${lib.getExe pkgs.losos-backup} run --if-due";
  };

  # Settings' "Back up now": a unit, so the run outlives the Settings window
  # and its log goes to the journal with the timer's.
  systemd.user.services.losos-backup-now = lib.recursiveUpdate unit {
    description = "Back up to the LosOS box now";
    serviceConfig.ExecStart = "${lib.getExe pkgs.losos-backup} run";
  };

  systemd.user.timers.losos-backup = {
    description = "Ask whether a backup to the LosOS box is due";
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnStartupSec = "10min";
      OnUnitActiveSec = "1h";
      RandomizedDelaySec = "5min";
    };
  };
}
