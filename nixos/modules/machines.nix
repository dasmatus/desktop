# The machine templates the image ships: disk images that a machine is made
# from (docs/hypervisor.md, "The guest template"), and the CDs of guest tools
# a machine that installs Windows or another distribution from its own CD is
# given beside it. The templates are losos.machines.guests: the LosOS guest
# (nixos/guest/default.nix), for a person's own apps, and whatever other
# modules add, such as losos-boxes.nix's LosOS boxes machine. Each is
# evaluated here as a NixOS system of its own with the image's package
# set, the way installer.nix evaluates the installer.
#
# Templates live under share/losos/machines, on XDG_DATA_DIRS, where derisk's
# Machines app looks for them, and `pm machine create --image` copies them
# from there into the person's own machines folder once, read-only, as the
# base each machine's disk is a layer over. They are in /usr, covered by its
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
  # Each template is its own NixOS system, named after its attribute in
  # losos.machines.guests.
  guests = lib.mapAttrs (
    name: module:
    import "${modulesPath}/../lib/eval-config.nix" {
      system = null;
      modules = [
        module
        { losos.guest.name = name; }
        # The image's own package set, overlay and all, so a package the
        # host and the guests share is one store path and one build.
        { nixpkgs.pkgs = pkgs; }
      ];
    }
  ) config.losos.machines.guests;

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
  # and all but the store in it is empty. QEMU reads a compressed qcow2 as a
  # backing file like any other, and pm copies it as it is.
  #
  # Beside each, <name>.apps: the apps the guest has when it is new, as its
  # own `losos-guest-run --list` would print them, read from the system
  # profile it boots. derisk offers a machine's apps while it is off from
  # the list the machine gave last, and before it has ever run, from this
  # one, so a machine made from a template has its apps in the palette at
  # once (derisk-machines' template_apps).
  templates =
    pkgs.runCommand "losos-machine-templates"
      {
        nativeBuildInputs = [
          pkgs.buildPackages.qemu-utils
          pkgs.buildPackages.gawk
          pkgs.buildPackages.gnused
        ];
      }
      (
        ''
          mkdir -p $out/share/losos/machines
        ''
        + lib.concatMapStrings (
          system:
          let
            inherit (system.config.losos.guest) name;
          in
          ''
            qemu-img convert -f raw -O qcow2 -c \
              ${system.config.system.build.image}/${system.config.image.fileName} \
              $out/share/losos/machines/${name}.qcow2
            LOSOS_GUEST_APPLICATIONS=${system.config.system.path}/share/applications \
              bash ${guestRun} --list > $out/share/losos/machines/${name}.apps
          ''
        ) (lib.attrValues guests)
      );

  # losos-guest-run as a build runs it, for the lists above: the user and
  # display are the guest's, and unused by --list.
  guestRun = pkgs.writeText "losos-guest-run" (
    builtins.replaceStrings [ "@user@" "@display@" ] [ "user" "wayland-0" ] (
      builtins.readFile ../guest/losos-guest-run.sh
    )
  );

  # The machines the host makes by itself (losos.machines.system), with
  # what `pm machine create` is told of each.
  systemMachines = lib.mapAttrsToList (
    name: machine:
    lib.escapeShellArgs (
      [
        name
        "${templates}/share/losos/machines/${machine.template}.qcow2"
        "--label"
        machine.label
      ]
      ++ lib.optionals (machine.memory != null) [
        "--memory"
        (toString machine.memory)
      ]
      ++ lib.optionals (machine.disk != null) [
        "--disk"
        (toString machine.disk)
      ]
    )
  ) config.losos.machines.system;
in
{
  options.losos.machines.guests = lib.mkOption {
    type = lib.types.attrsOf lib.types.deferredModule;
    description = ''
      The machine templates built from nixos/guest/common.nix, by name: each
      is a NixOS module that imports common.nix, and ships as
      share/losos/machines/<name>.qcow2, the name pm records as a machine's
      template. A module that needs a machine of its own for something adds
      one here, as losos-boxes.nix's LosOS boxes machine does.
    '';
  };

  options.losos.machines.system = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options = {
          template = lib.mkOption {
            type = lib.types.str;
            description = "The template it is made from, a name in losos.machines.guests.";
          };
          label = lib.mkOption {
            type = lib.types.enum [
              "red"
              "orange"
              "yellow"
              "green"
              "gray"
              "blue"
              "purple"
              "black"
            ];
            description = "The colour its windows are framed in.";
          };
          memory = lib.mkOption {
            type = lib.types.nullOr lib.types.ints.positive;
            default = null;
            description = "Its memory in MiB, or pm's default.";
          };
          disk = lib.mkOption {
            type = lib.types.nullOr lib.types.ints.positive;
            default = null;
            description = "Its disk in GiB, or pm's default.";
          };
        };
      }
    );
    default = { };
    description = ''
      Machines the OS makes for every person, by name, for a part of the OS
      that runs in a machine of its own rather than on the host, such as
      flatpak-machine.nix's Flatpak machine. Each is made once in a person's
      own machines, at the first sign-in that declares it, the way they
      would make it with `pm machine create`; one they removed since is not
      made again.
    '';
  };

  options.losos.machines.templates = lib.mkOption {
    type = lib.types.bool;
    default = true;
    description = ''
      Whether the image carries its machine templates. Off only where a
      system boots the image for something else: a template is a whole
      guest's store, and every disk built from the image copies it again.
    '';
  };

  config = {
    # The LosOS guest, for a person's own apps.
    losos.machines.guests.losos-guest = ../guest;

    environment.systemPackages = [ tools ] ++ lib.optional config.losos.machines.templates templates;
    environment.pathsToLink = [ "/share/losos/machines" ];

    assertions = lib.mapAttrsToList (name: machine: {
      assertion = config.losos.machines.guests ? ${machine.template};
      message = "losos.machines.system.${name}: there is no template ${machine.template} in losos.machines.guests";
    }) config.losos.machines.system;

    # Makes losos.machines.system's machines through pm, in each person's
    # own machines (~/.local/share/pm/machines), when they sign in: a
    # machine is its owner's, as quickemu runs it, so the OS's are made per
    # person like the ones they make themselves. pm runs a plugin only when
    # it is signed by a key its user trusts, and the person may have no key
    # or plugin set up: this unit keeps a key of its own under their state
    # directory, made once on this computer and never in the image (a key
    # the image carried would be one every LosOS computer trusted), with a
    # pm configuration of its own beside it, and signs the image's quickemu
    # plugin with it at every start, since an update may bring a new one.
    # Which machines it made is kept there too, so one its owner removed in
    # the Machines app stays removed.
    #
    # A system service made them once, as root, while libvirt kept the
    # machines in its system daemon for everyone; quickemu has no such
    # place, and a machine in root's home would be nobody's to open.
    #
    # Without the templates in the image there is nothing to make them from.
    systemd.user.services.losos-system-machines =
      lib.mkIf (config.losos.machines.system != { } && config.losos.machines.templates)
        {
          description = "Make the machines the system runs parts of itself in";
          # The login screen's own user, and any other system account that
          # gets a user manager, is not a person.
          unitConfig.ConditionUser = "!@system";
          wantedBy = [ "default.target" ];
          # pm and what the quickemu plugin's steps run.
          path = [
            pkgs.pm
            pkgs.quickemu
            pkgs.qemu-utils
            pkgs.coreutils
            pkgs.findutils
            config.systemd.package
          ];
          environment.NO_COLOR = "1";
          serviceConfig = {
            Type = "oneshot";
            RemainAfterExit = true;
          };
          script = ''
            state="''${XDG_STATE_HOME:-$HOME/.local/state}/losos/system-machines"
            machines="''${XDG_DATA_HOME:-$HOME/.local/share}/pm/machines"
            # Its own pm configuration, key and plugins; the machines still
            # go where the person's own do, under XDG_DATA_HOME.
            export XDG_CONFIG_HOME="$state/config"
            plugins="$XDG_CONFIG_HOME/pm/plugins"
            mkdir -p -m 0700 "$state"
            [ -e "$XDG_CONFIG_HOME/pm/signing.key" ] || pm keygen
            mkdir -p "$plugins" "$state/made"
            install -m 0644 ${pkgs.pm-plugins}/share/pm/plugins/quickemu.wasm "$plugins/quickemu.wasm"
            pm sign "$plugins/quickemu.wasm"

            make_machine() {
              name=$1
              shift
              if [ -e "$machines/$name/machine.json" ] || [ -e "$state/made/$name" ]; then
                return 0
              fi
              image=$1
              shift
              pm machine create "$name" --image "$image" "$@"
              touch "$state/made/$name"
            }
            ${lib.concatMapStrings (args: "make_machine ${args}\n") systemMachines}
          '';
        };

    # Buildable on their own whether or not the image carries them.
    system.build = {
      guestSystems = guests;
      machineTemplates = templates;
      machineTools = tools;
    };
  };
}
