# The start-up sound: the launch film's closing two bars, which play under
# its end card as the screen shuts into the boot line and the salmon swims
# in, play as the machine starts, and backwards as it shuts down or
# restarts. The film's end card is the boot screen, so the sound goes with
# the screen it was made for: Plymouth's salmon and line (splash.nix).
#
#   start   losos-boot-sound starts when the kernel has found a sound card
#           (sound.target), while Plymouth still shows the salmon, and
#           before anyone can log in, so nothing holds the card yet.
#   stop    its ExecStop plays the track reversed once every session and
#           the login screen are gone, which leaves the card free again,
#           under Plymouth's "shutting down" screen. Shut down from the
#           desktop, derisk has already pressed its screen into that line.
#
# It plays straight to ALSA's default card: PipeWire runs per user and is
# not there yet at boot, nor any more at shutdown. ALSA keeps each card's
# volume across boots (hardware.alsa.enablePersistence below), so the sound
# is as loud as the machine was left. A machine without a sound card never
# reaches sound.target and plays nothing, and a card that refuses the sound
# changes nothing either: both commands may fail.
#
# The track is nixos/branding/end-track.flac (CREDITS.md): the film's
# soundtrack from 34 s, its bar 18, to its end at 37 s, with a 5 ms fade-in
# so the bars before it don't click.
{ lib, pkgs, ... }:

let
  aplay = lib.getExe' pkgs.alsa-utils "aplay";

  # aplay reads WAV, not FLAC. The reversed copy fades out over its last
  # 30 ms, which were the forward track's first, so it doesn't end on a
  # click either.
  sounds = pkgs.runCommand "losos-boot-sound" { nativeBuildInputs = [ pkgs.sox ]; } ''
    mkdir $out
    sox ${../branding/end-track.flac} $out/start.wav
    sox ${../branding/end-track.flac} $out/stop.wav reverse fade 0 -0 0.03
  '';
in
{
  # alsa-store restores each card's mixer when it appears and stores it at
  # shutdown. Without it a card comes up at its driver's defaults, which are
  # often muted, until a session's WirePlumber sets it.
  hardware.alsa.enablePersistence = true;
  # NixOS starts alsa-store with multi-user.target, and `alsactl restore`
  # fails when no card is there yet: on a machine without one, and on one
  # whose card the kernel finds late, which leaves a failed unit that
  # boot-complete.target's health check counts. With the first card, as the
  # sound is, it only runs where it has something to restore.
  systemd.services.alsa-store = {
    wantedBy = lib.mkForce [ "sound.target" ];
    after = [ "sound.target" ];
  };

  systemd.services.losos-boot-sound = {
    description = "LosOS start-up and shutdown sound";
    wantedBy = [ "sound.target" ];
    # Stops before alsa-store stores the mixer, and after every session.
    after = [
      "sound.target"
      "alsa-store.service"
    ];
    before = [ "systemd-user-sessions.service" ];
    unitConfig.ConditionVirtualization = "!container";
    serviceConfig = {
      # A simple service counts as started at once, so the sound never holds
      # up the boot; RemainAfterExit keeps it active for its ExecStop.
      Type = "simple";
      RemainAfterExit = true;
      ExecStart = "-${aplay} -q ${sounds}/start.wav";
      ExecStop = "-${aplay} -q ${sounds}/stop.wav";
      TimeoutStopSec = 10;
    };
  };
}
