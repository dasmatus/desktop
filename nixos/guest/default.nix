# The guest: the machine template the image ships for its virtual machine
# desktop (docs/hypervisor.md). machines.nix in the OS evaluates this with
# the image's own package set and turns `system.build.image` into
# share/losos/machines/losos-guest.qcow2, which derisk's Machines app offers
# when a machine is made and `pm machine create` installs into libvirt.
#
# A guest has no screen and no session. Its apps' windows go to the host's
# desktop through one waypipe server, which the guest's user runs from boot
# and which carries each app's Wayland connection to the host over vsock; the
# host knows which machine a window is from by the vsock address libvirt
# gave it. The host starts apps here through the QEMU guest agent and
# `losos-guest-run`, and nothing else in the guest listens for the host.
{
  config,
  lib,
  pkgs,
  modulesPath,
  ...
}:

let
  # The one account in the guest, whose apps the desktop shows. Nobody logs
  # in as it: it has no password, and its systemd manager runs from boot
  # (linger), which is where the waypipe server and every app run.
  user = "user";

  efiArch = pkgs.stdenv.hostPlatform.efiArch;

  guestRun = pkgs.writeShellApplication {
    name = "losos-guest-run";
    runtimeInputs = with pkgs; [
      coreutils
      gawk
      gnused
      systemd
    ];
    text = builtins.replaceStrings [ "@user@" "@display@" ] [ user "wayland-0" ] (
      builtins.readFile ./losos-guest-run.sh
    );
  };

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
    "${modulesPath}/profiles/qemu-guest.nix"
    "${modulesPath}/image/repart.nix"
    # The same browser and Flatpak setup as the host, Bazaar and Flathub
    # included, so apps a person installs in a machine come from where the
    # host's come from.
    ../modules/danube.nix
    ../modules/flatpak.nix
  ];

  system.stateVersion = "26.05";
  networking.hostName = "losos-guest";

  # Nothing in the guest builds or switches: it is a template, and a machine
  # made from it keeps what its owner changes on its own disk. Only a
  # default, because Home Manager needs a working Nix to activate a home
  # (home.nix turns it on where the flake adds Home Manager).
  nix.enable = lib.mkDefault false;
  documentation.enable = false;

  # The firmware boots the UKI straight from the ESP's fallback path. No boot
  # loader, because a machine never chooses between generations: a new
  # template makes new machines.
  boot.loader.grub.enable = false;
  boot.initrd.systemd.enable = true;
  fileSystems."/" = {
    device = "/dev/disk/by-partlabel/losos-guest";
    fsType = "ext4";
    # ext4 grows while mounted, once repart has made the partition larger.
    autoResize = true;
  };

  # A machine's disk is larger than the template that is its base, and the
  # root partition takes the rest of it the first time the machine boots.
  # The disk is always vda: pm's libvirt plugin gives a machine one virtio
  # disk and nothing else. Partitions repart has no definition for, the
  # ESP, are left as they are.
  boot.initrd.systemd.repart = {
    enable = true;
    device = "/dev/vda";
  };
  systemd.repart.partitions.root.Type = "root";

  image.repart = {
    enable = true;
    name = "losos-guest";
    partitions = {
      esp = {
        contents."/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source =
          "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          SizeMinBytes = "128M";
        };
      };
      root = {
        storePaths = [ config.system.build.toplevel ];
        repartConfig = {
          Type = "root";
          Format = "ext4";
          Label = "losos-guest";
          # As small as the store allows: a machine's disk is as large as
          # its owner asks (`pm machine create --disk`, 32 GiB unless told
          # otherwise), and the first boot grows the root into it, below.
          Minimize = "guess";
        };
      };
    };
  };

  services.qemuGuest.enable = true;

  # DHCP on libvirt's default network, which hands out the address and the
  # host's resolver.
  networking.useNetworkd = true;

  users.users.${user} = {
    isNormalUser = true;
    uid = 1000;
    linger = true;
  };

  # flatpak's own rule lets an active, local wheel member install for the
  # whole system. The guest's user is never in an active session (nobody
  # logs in; its apps run under its systemd manager), so it is given
  # Flatpak's actions here instead. The machine is its owner's to fill.
  security.polkit.extraConfig = ''
    polkit.addRule(function (action, subject) {
      if (action.id.indexOf("org.freedesktop.Flatpak.") === 0 && subject.user === "${user}") {
        return polkit.Result.YES;
      }
      return polkit.Result.NOT_HANDLED;
    });
  '';

  # The guest's display: a waypipe server whose socket, wayland-0 in the
  # user's runtime directory, every app connects to as if it were a
  # compositor. Each connection becomes a vsock stream to the host (address
  # 2, port 7350), where the desktop's gate takes it. --no-gpu, because the
  # guest has no GPU and its apps draw into shared memory (llvmpipe and
  # lavapipe below), which is what waypipe then carries.
  systemd.user.services.waypipe = {
    description = "Show this machine's apps on the host's desktop";
    wantedBy = [ "default.target" ];
    serviceConfig = {
      ExecStart = lib.escapeShellArgs [
        (lib.getExe pkgs.waypipe)
        "--vsock"
        "--socket"
        "2:7350"
        "--no-gpu"
        "--display"
        "wayland-0"
        "server"
        "--"
        "${pkgs.coreutils}/bin/sleep"
        "infinity"
      ];
      Restart = "always";
    };
  };
  # Every unit of the user's manager draws on that socket: the apps
  # losos-guest-run starts and the portal's dialogs alike.
  systemd.user.settings.Manager.DefaultEnvironment =
    "WAYLAND_DISPLAY=wayland-0 XDG_SESSION_TYPE=wayland";

  # Mesa's software renderers, OpenGL and Vulkan: the guest has no GPU, and
  # derisk's GPUI apps need Vulkan.
  hardware.graphics.enable = true;

  # flatpak.nix needs a portal. GTK's draws its dialogs (open a file, pick
  # an app) through the guest's waypipe like any other window.
  xdg.portal = {
    enable = true;
    extraPortals = [ pkgs.xdg-desktop-portal-gtk ];
    config.common.default = "gtk";
  };

  # A captive portal is the host's to deal with: the guest is behind
  # libvirt's NAT.
  losos.captivePortal.enable = false;

  # Hack Nerd Font for derisk-terminal, as on the host (modules/desktop.nix).
  fonts.enableDefaultPackages = true;
  fonts.packages = [ pkgs.nerd-fonts.hack ];

  environment.systemPackages = [
    guestRun
    calculator
    # The host's terminal, without the rest of derisk. With no GPU in the
    # machine it draws in software into the shared memory waypipe carries.
    pkgs.derisk.terminal
    pkgs.adwaita-icon-theme
  ];
}
