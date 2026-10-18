# Windows app support (docs/windows-apps.md): Windows apps as windows of
# their own on the desktop, through WinBoat. Each person who opens it gets
# a Windows of their own, which sets itself up the first time
# (nixos/pkgs/windows-app-support.nix) and runs in a rootless Podman
# container of theirs: dockur/windows, a QEMU with KVM, and FreeRDP on the
# host for each app's window.
#
# WinApps did this before, against a Windows machine a person installed
# themselves from the Windows tools CD and then described to it by hand.
# WinBoat installs the Windows it serves, so nothing is described.
#
# WinBoat and Windows are x86-64 only.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  podman = config.virtualisation.podman.package;

  # `podman` for everyone, which runs the real one for anyone but root. A
  # container of root's would run with the whole system's privilege, so
  # here there are none: Podman is rootless only, as pm's podman plugin is.
  # Root inside a person's own rootless namespace, which Podman becomes as
  # it works, has a uid_map of its own and is let through; Podman re-runs
  # itself by its real path in any case.
  rootlessPodman = pkgs.writeShellScriptBin "podman" ''
    if [ "$EUID" = 0 ] && [ "$(${pkgs.coreutils}/bin/tr -s ' ' < /proc/self/uid_map)" = " 0 0 4294967295" ]; then
      echo "podman: containers here are rootless only; run podman as the person whose containers they are" >&2
      exit 125
    fi
    exec ${podman}/bin/podman "$@"
  '';

  # systemd-homed's range, from which every person here gets their UID
  # (accounts.nix).
  homedUids = lib.range 60001 60513;

  # A rootless container maps its root and every other user inside onto
  # the person's subordinate IDs, which /etc/subuid and /etc/subgid list.
  # NixOS writes them for the users it makes, but nobody here is one of
  # those: people are homed's, made after the image was built. shadow's
  # newuidmap and Podman both find a person's range by UID as well as by
  # name, so each of homed's UIDs gets 65536 IDs of its own, from 100000
  # up, as NixOS gives its own users, whether or not anyone has that UID.
  subordinateIds = lib.concatMapStrings (
    uid: "${toString uid}:${toString (100000 + (uid - 60001) * 65536)}:65536\n"
  ) homedUids;

in
lib.mkIf pkgs.stdenv.hostPlatform.isx86_64 {
  environment.systemPackages = [
    pkgs.windows-app-support
    # Ahead of Podman's own `podman` (above).
    (lib.hiPrio rootlessPodman)
  ];

  # Podman, rootless: each person's Windows is their own container, with
  # their own image store in their home. WinBoat could use Docker too, whose
  # group would make everyone in it root; it is patched to Podman only, and
  # runs it through pm (nixos/pkgs/windows-app-support.nix).
  virtualisation.podman.enable = true;
  # Nor root's Podman API, whose socket NixOS opens to the podman group: a
  # rootful daemon by another name. Each person's own socket stays.
  systemd.sockets.podman.wantedBy = lib.mkForce [ ];
  environment.etc.subuid.text = subordinateIds;
  environment.etc.subgid.text = subordinateIds;

  # The setup, as each person's own unit, which the app starts when its
  # person has no Windows yet and which carries on while the app is closed:
  # Windows downloads and installs itself for most of an hour. Podman and
  # newuidmap come from the PATH the user manager gives every unit.
  systemd.user.services.windows-app-support-setup = {
    description = "Set up Windows for Windows app support";
    serviceConfig = {
      Type = "exec";
      ExecStart = lib.getExe pkgs.windows-app-support.setup;
    };
  };
}
