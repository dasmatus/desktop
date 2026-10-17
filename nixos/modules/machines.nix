# The machine templates the image ships: disk images that a machine is made
# from (docs/hypervisor.md, "The guest template"), and the CDs of guest tools
# a machine that installs Windows or another distribution from its own CD is
# given beside it. Today there is one, the
# LosOS guest, nixos/guest/ evaluated here as a NixOS system of its own with
# the image's package set, the way installer.nix evaluates the installer.
#
# Templates live under share/losos/machines, on XDG_DATA_DIRS, where derisk's
# Machines app looks for them, and `pm machine create --image` takes them
# from there into libvirt's default pool. They are in /usr, covered by its
# verity hash like the rest of the OS, so a template is exactly what the
# release built; an update brings a new one, which new machines are made
# from while machines made earlier keep the template they were made from.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  guest = import "${modulesPath}/../lib/eval-config.nix" {
    system = null;
    modules = [
      ../guest
      # The image's own package set, overlay and all, so a package the host
      # and the guest share is one store path and one build.
      { nixpkgs.pkgs = pkgs; }
    ]
    ++ config.losos.machines.guestModules;
  };

  inherit (guest.config.system.build) image;

  # A CD of `dir`'s files, which Windows and Linux both read: Joliet for
  # Windows' long names, Rock Ridge for Linux's modes, so setup.sh stays
  # executable.
  cd =
    name: volume: dir:
    pkgs.runCommand name { nativeBuildInputs = [ pkgs.buildPackages.xorriso ]; } ''
      xorriso -as mkisofs -quiet -J -joliet-long -R -V ${volume} -o $out ${dir}
    '';

  # The CD a Windows machine is made with (docs/hypervisor.md, "Windows"),
  # where pm looks for it: virtio's disk and network drivers for Windows 10
  # and 11 and the answer file that has Setup load them, then the rest of
  # virtio's drivers, the QEMU guest agent and WinApps' Windows half at the
  # first sign-in. Only what that needs from virtio-win, not its drivers for
  # every Windows since XP, which would be most of a GiB in /usr.
  windowsTools =
    let
      drivers = [
        "viostor"
        "NetKVM"
      ];
      windows = [
        "w11"
        "w10"
      ];
      # Every letter Setup may give this CD, behind the Windows CD and
      # anything else it finds first.
      letters = lib.stringToCharacters "DEFGHIJK";
      paths = lib.concatMap (
        letter:
        lib.concatMap (driver: map (version: "${letter}:\\${driver}\\${version}\\amd64") windows) drivers
      ) letters;
      driverPaths = lib.concatStrings (
        lib.imap1 (key: path: ''
          <PathAndCredentials wcm:action="add" wcm:keyValue="${toString key}">
            <Path>${path}</Path>
          </PathAndCredentials>
        '') paths
      );
      answers = pkgs.replaceVars ../guest-tools/windows/autounattend.xml { inherit driverPaths; };
      tree = pkgs.runCommand "losos-windows-tools-tree" { } ''
        mkdir -p $out/losos/winapps
        cp ${answers} $out/autounattend.xml
        cp ${pkgs.virtio-win}/virtio-win-guest-tools.exe ${pkgs.virtio-win}/virtio-win_license.txt $out/
        ${lib.concatMapStrings (
          driver:
          lib.concatMapStrings (version: ''
            mkdir -p $out/${driver}/${version}
            cp -r ${pkgs.virtio-win}/${driver}/${version}/amd64 $out/${driver}/${version}/
          '') windows
        ) drivers}
        # A batch file with Unix line ends can lose its labels and the
        # line after a `call`; cmd reads CRLF as written.
        sed 's/$/\r/' ${../guest-tools/windows/setup.cmd} > $out/losos/setup.cmd
        # WinApps' own Windows setup, as its repository has it (CRLF
        # and UTF-16 already), less Container.reg, which is for its
        # docker flavour and would set Windows' clock to UTC under a
        # machine whose clock is local time.
        for file in install.bat RDPApps.reg TimeSync.ps1 NetProfileCleanup.ps1; do
          cp ${pkgs.winapps.src}/oem/$file $out/losos/winapps/
        done
      '';
    in
    cd "windows-tools.iso" "LOSOS_WINTOOLS" tree;

  # The CD another distribution's machine is made with (docs/hypervisor.md,
  # "Other distributions"): setup.sh, run once in the installed system, and
  # the losos-guest-run it installs, which reads the user setup.sh was given.
  linuxTools = cd "linux-tools.iso" "LOSOS_TOOLS" (
    pkgs.runCommand "losos-linux-tools-tree" { } ''
      mkdir $out
      install -m 0755 ${../guest-tools/linux/setup.sh} $out/setup.sh
      {
        echo '#!/bin/bash'
        echo 'set -euo pipefail'
        sed -e 's|^user=@user@$|user=$(cat /etc/losos-guest-user)|' \
          -e 's|^display=@display@$|display=losos-wayland|' \
          ${../guest/losos-guest-run.sh}
      } > $out/losos-guest-run
      chmod 0755 $out/losos-guest-run
    ''
  );

  # Where pm looks for each, under XDG_DATA_DIRS (pm's guest_tools).
  tools = pkgs.runCommand "losos-machine-tools" { } (
    ''
      mkdir -p $out/share/pm/machines
      ln -s ${linuxTools} $out/share/pm/machines/linux-tools.iso
    ''
    + lib.optionalString pkgs.stdenv.hostPlatform.isx86_64 ''
      ln -s ${windowsTools} $out/share/pm/machines/windows-tools.iso
    ''
  );

  # qcow2, compressed: the raw image is as large as a machine's whole disk,
  # and all but the store in it is empty. libvirt reads a compressed qcow2
  # as a backing file like any other, and pm uploads it into the pool as it
  # is.
  templates =
    pkgs.runCommand "losos-machine-templates"
      {
        nativeBuildInputs = [ pkgs.buildPackages.qemu-utils ];
      }
      ''
        mkdir -p $out/share/losos/machines
        qemu-img convert -f raw -O qcow2 -c \
          ${image}/${guest.config.image.fileName} \
          $out/share/losos/machines/losos-guest.qcow2
      '';
in
{
  options.losos.machines.templates = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = ''
      Whether the image carries its machine templates. Off only where a
      system boots the image for something else: a template is a whole
      guest's store, and every disk built from the image copies it again.
    '';
  };

  # The flake hands Home Manager in here (flake.nix), since a module of the
  # OS has no way to reach the flake's inputs itself.
  options.losos.machines.guestModules = lib.mkOption {
    type = lib.types.listOf lib.types.deferredModule;
    default = [ ];
    description = ''
      More modules for the guest template, beside nixos/guest.
    '';
  };

  config = {
    environment.systemPackages = [ tools ] ++ lib.optional config.losos.machines.templates templates;
    environment.pathsToLink = [ "/share/losos/machines" ];

    # Buildable on their own whether or not the image carries them.
    system.build = {
      guestSystem = guest;
      machineTemplates = templates;
      machineTools = tools;
    };
  };
}
