# Legacy BIOS PCs, started by GRUB from the MBR (docs/boot-loaders.md).
#
# UEFI firmware starts GRUB from the ESP (grub.nix). This is for x86_64 PCs
# whose firmware has no UEFI, or has it turned off: the same disk, the same
# /usr slots, the same menu and theme and the same updates, started by GRUB
# from the MBR instead. Every disk the image or the installer lays out
# carries both, so one disk starts on either kind of firmware.
#
#   BIOS -> MBR (GRUB's boot.img) -> core.img in the BIOS boot partition,
#     menu and theme built in -> kernel + initrd + command line of the
#     newest UKI's version -> the same initrd and system as under UEFI
#
# BIOS GRUB cannot run a UKI, which is an EFI program, so every release
# also carries each UKI's kernel, initrd and command line as files of their
# own, which sysupdate installs beside the UKI (transfers below). GRUB still
# reads versions and tries from the UKIs' names, and counts tries in
# grubenv, as it does on UEFI; losos-grub-bless (grub.nix) writes the
# outcome back into those names, where sysupdate reads it.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.losos;
  inherit (cfg) arch version;
  id = config.system.image.id;
  inherit (config.system.boot.loader) ukiFile;
  inherit (config.system.build) grub;

  # core.img for the BIOS boot partition, with grub.cfg, the theme and
  # what they use built in: grub-image.nix's BIOS set (disks, GPT, linux,
  # regexp, serial, the memdisk), plus FAT to find the files and loadenv
  # for the counter. The early config sets losos_disk to $root, which
  # before the menu is the disk the BIOS started GRUB from.
  core = import ./grub-image.nix {
    inherit pkgs;
    inherit (grub) menu art;
    format = "i386-pc";
    output = "core.img";
    early = ''
      set losos_disk="$root"
      normal (memdisk)/grub.cfg
    '';
    modules = [
      "probe"
      "fat"
      "true"
      "loadenv"
      "reboot"
      "halt"
      "sleep"
    ];
  };

  # boot.img for the MBR, and core.img.
  bootCode = pkgs.runCommand "losos-grub-bios" { } ''
    mkdir $out
    cp ${core}/core.img $out/core.img
    cp ${pkgs.grub2}/lib/grub/i386-pc/boot.img $out/boot.img
  '';

  install = pkgs.writeShellApplication {
    name = "losos-grub-bios-install";
    runtimeInputs = with pkgs; [
      coreutils
      util-linux
      jq
    ];
    text = ''
      boot_code=${bootCode}
    ''
    + builtins.readFile ./grub-bios-install.sh;
  };

  # The root partition on the disk whose ESP GRUB started the kernel from,
  # which the initrd's udev rule below links here.
  loaderRoot = "/dev/disk/by-loader/root";

  partx = "${pkgs.util-linux}/bin/partx";

  # Succeeds when the disk $1 has a partition whose UUID is $2: the udev
  # rule's test that a root partition sits on the same disk as the ESP GRUB
  # named. It reads the disk's partition table itself rather than asking
  # udev, which may not have seen the ESP yet.
  onLoaderDisk = pkgs.writeScript "losos-on-loader-disk" ''
    #!/bin/sh
    tr=${pkgs.coreutils}/bin/tr
    want=$(printf '%s' "$2" | $tr A-F a-f)
    for uuid in $(${partx} --show --noheadings --output UUID "$1" | $tr A-F a-f); do
      [ "$uuid" = "$want" ] && exit 0
    done
    exit 1
  '';

  loaderRootRule = ''
    SUBSYSTEM=="block", ENV{DEVTYPE}=="partition", ENV{ID_PART_ENTRY_NAME}=="${arch.root}", IMPORT{cmdline}="losos.esp", ENV{losos.esp}=="?*", PROGRAM="${onLoaderDisk} $parent $env{losos.esp}", SYMLINK+="${lib.removePrefix "/dev/" loaderRoot}"
  '';

  # The UKI's kernel, initrd and command line, for GRUB to start what the
  # UKI would. Cut from the finished UKI rather than taken from the
  # configuration, because only the finished UKI's command line carries the
  # usrhash= of the /usr it belongs to.
  #
  # root=gpt-auto is replaced: systemd finds the root partition by type on
  # the disk the boot loader names in LoaderDevicePartUUID, an EFI variable,
  # and a BIOS has none to set. In its place GRUB names the ESP it started
  # the kernel from, as losos_esp, and the initrd's rule below finds root on
  # that ESP's disk. A GRUB from before losos_esp sets nothing, and the line
  # falls back to the root partition's label, which repart gives it from its
  # type (disk.nix gives it none of its own): right with one LosOS disk in
  # the machine, a coin toss with two.
  files =
    pkgs.runCommand "${id}_${version}-bios"
      {
        nativeBuildInputs = with pkgs.buildPackages; [
          jq
          systemdUkify
        ];
      }
      ''
        cmdline=$(ukify --json=short inspect ${config.system.build.uki}/${ukiFile} \
          | jq -er '.".cmdline".text' | tr -d '\0\n')
        case " $cmdline " in
          *" usrhash="*" root=gpt-auto "* | *" root=gpt-auto "*" usrhash="*) ;;
          *) echo "unexpected UKI command line: $cmdline" >&2; exit 1 ;;
        esac
        # It goes into a double-quoted GRUB string, where these would mean
        # something.
        case "$cmdline" in
          *[\"\$\\\']*) echo "UKI command line needs quoting: $cmdline" >&2; exit 1 ;;
        esac
        cmdline=''${cmdline/root=gpt-auto/root=\$losos_root}

        mkdir $out
        cp ${config.boot.uki.settings.UKI.Linux} $out/${id}_${version}.linux
        cp ${config.boot.uki.settings.UKI.Initrd} $out/${id}_${version}.initrd
        {
          echo 'if [ -n "$losos_esp" ]; then'
          echo '  set losos_root="${loaderRoot} losos.esp=$losos_esp"'
          echo 'else'
          echo '  set losos_root="PARTLABEL=${arch.root}"'
          echo 'fi'
          printf 'set losos_cmdline="%s"\n' "$cmdline"
        } > $out/${id}_${version}.grub
      '';

  # Each of those three as a sysupdate transfer, installed beside the UKI in
  # $BOOT/EFI/Linux, where grub.cfg reads only *.efi names and finds these
  # by the UKI's.
  # They belong to the same version set as the UKI and the /usr slot
  # (update.nix), so a version is installed with all of them or not at all.
  transfer = suffix: {
    Transfer.ProtectVersion = "%A";
    Source = {
      Type = "url-file";
      Path = cfg.update.baseUrl;
      MatchPattern = "${id}_@v.${suffix}";
    };
    Target = {
      Type = "regular-file";
      Path = "/EFI/Linux";
      PathRelativeTo = "boot";
      MatchPattern = "${id}_@v.${suffix}";
      Mode = "0444";
      InstancesMax = 2;
    };
  };

  # systemd-gpt-auto-generator mounts the ESP only on an EFI boot: without
  # LoaderDevicePartUUID it cannot tell which ESP the boot came from. A BIOS
  # boot came from the disk root is on, so this mounts that disk's ESP at
  # /efi, where gpt-auto would have, and sysupdate and bootctl find $BOOT
  # there. On a UEFI boot it writes nothing and gpt-auto does the job.
  espGenerator = pkgs.writeShellScript "losos-bios-esp-generator" ''
    [ -d /sys/firmware/efi ] && exit 0
    PATH=${lib.makeBinPath [ pkgs.util-linux ]}
    root=$(findmnt --noheadings --output SOURCE --nofsroot /) || exit 0
    disk=$(lsblk --noheadings --nodeps --output PKNAME "$root") || exit 0
    [ -n "$disk" ] || exit 0
    esp=$(lsblk --noheadings --list --output PARTTYPE,PARTUUID "/dev/$disk" \
      | while read -r type uuid; do
          if [ "$type" = c12a7328-f81f-11d2-ba4b-00a0c93ec93b ]; then
            echo "$uuid"
            break
          fi
        done)
    [ -n "$esp" ] || exit 0
    cat > "$1/efi.mount" <<EOF
    # Written by losos-bios-esp-generator (bios.nix).
    [Unit]
    Description=EFI System Partition
    Documentation=man:systemd-gpt-auto-generator(8)
    Before=local-fs.target

    [Mount]
    What=/dev/disk/by-partuuid/$esp
    Where=/efi
    Type=vfat
    Options=umask=0077,noexec,nosuid,nodev
    EOF
    mkdir -p "$1/local-fs.target.wants"
    ln -sf ../efi.mount "$1/local-fs.target.wants/efi.mount"
  '';
in
lib.mkIf (arch.name == "x86_64") {
  system.build = {
    grubBiosInstall = install;
    biosBootFiles = files;
  };

  # Where core.img goes: 1M, the size grub-install asks for, and far more
  # than the ~300K core.img is with the theme inside. Before slot A's /usr, which grows into the
  # space behind it on first boot (disk.nix). disk.nix has no definition of
  # its own for it, and repart leaves alone a partition no definition
  # matches.
  image.repart.partitions."05-bios-boot".repartConfig = {
    Type = "21686148-6449-6e6f-744e-656564454649";
    Label = "BIOS boot";
    SizeMinBytes = "1M";
    SizeMaxBytes = "1M";
  };

  systemd.sysupdate.transfers = {
    "40-bios-linux" = transfer "linux";
    "41-bios-initrd" = transfer "initrd";
    "42-bios-grub" = transfer "grub";
  };

  systemd.generators.losos-bios-esp = espGenerator;

  # What LoaderDevicePartUUID gives gpt-auto under UEFI: root on the disk
  # the boot came from, so another LosOS disk in the same machine is never
  # mounted in its place. losos.esp= is on the command line only when BIOS
  # GRUB put it there.
  # The rule is in the running system too, so the link the command line
  # names is still there after the switch to it.
  boot.initrd.services.udev.rules = loaderRootRule;
  services.udev.extraRules = loaderRootRule;
  boot.initrd.systemd = {
    # The script, and the store paths it names: coreutils is in every
    # initrd, partx is not.
    storePaths = [ onLoaderDisk ];
    extraBin.partx = partx;
  };
}
