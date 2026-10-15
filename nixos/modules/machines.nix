# The machine templates the image ships: disk images that a machine is made
# from (docs/hypervisor.md, "The guest template"). Today there is one, the
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
    ];
  };

  inherit (guest.config.system.build) image;

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

  config = {
    environment.systemPackages = lib.mkIf config.losos.machines.templates [ templates ];
    environment.pathsToLink = [ "/share/losos/machines" ];

    # Buildable on their own whether or not the image carries them.
    system.build = {
      guestSystem = guest;
      machineTemplates = templates;
    };
  };
}
