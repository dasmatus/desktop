# Boot the real image -- UEFI, GRUB, the UKI, a disk first boot has to
# lay out -- and check that each systemd piece this OS is built on is doing its
# job, not merely installed.
#
# The pm tree could not do this at all: nothing there had ever booted, and
# docs/boot.md said so. This test is the first thing in the repository that
# can fail because the design is wrong rather than because a build is.
{ self }:

{ lib, ... }:

let
  image = import ./image.nix { inherit self; };
in
{
  name = "losos-desktop-boot";

  # The module adds its own overlay, which the test framework's shared,
  # read-only package set would refuse.
  node.pkgsReadOnly = false;

  nodes.machine = image.node;

  testScript =
    { nodes, ... }:
    image.disk nodes.machine
    + ''

      machine.wait_for_unit("multi-user.target")

      with subtest("first boot laid out the disk with systemd-repart"):
          machine.succeed("findmnt --kernel --types ext4 /")
          machine.succeed("findmnt --kernel /home")
          # slot B exists and is waiting for sysupdate
          machine.succeed("sfdisk --json /dev/vda | grep -c '\"name\": \"_empty\"' | grep -qx 2")
          machine.succeed("swapon --show=NAME --noheadings | grep -q dm-")

      with subtest("GRUB chainloaded the UKI and named the disk it came from"):
          # bli's LoaderInfo, and the LoaderDevicePartUUID gpt-auto found root
          # and the ESP by (grub.nix).
          loader = machine.succeed(
              "tail -c +5 /sys/firmware/efi/efivars/LoaderInfo-4a67b082-0a4c-41cf-b6c7-440b29bb8c4f | tr -d '\\0'"
          )
          assert loader.startswith("GRUB"), loader
          machine.succeed("grep -q 'usrhash=' /proc/cmdline")
          esp = machine.succeed("bootctl --print-esp-path").strip()
          machine.succeed(f"test -f {esp}/EFI/Linux/losos-desktop_1.efi")
          machine.succeed(f"test -f {esp}/EFI/losos/grubenv")
          # The bless runs after boot-complete.target, which comes after
          # multi-user.target, so it can still be queued here; waiting
          # still fails if its job was dropped or it failed.
          machine.wait_for_unit("losos-grub-bless.service")

      with subtest("/usr is dm-verity and the store lives on it"):
          assert "ACTIVE" in machine.succeed("dmsetup info --target verity usr")
          machine.succeed("df --output=source /nix/store | tail -n1 | grep -qx /dev/mapper/usr")

      with subtest("the systemd services the design rests on are running"):
          for unit in [
              "systemd-homed.service",
              "systemd-userdbd.socket",
              "systemd-networkd.service",
              "systemd-resolved.service",
              "systemd-oomd.service",
              "systemd-factory-reset.socket",
              "systemd-sysupdate.timer",
              "display-manager.service",
          ]:
              machine.wait_for_unit(unit)
          machine.fail("systemctl is-active NetworkManager.service")

      with subtest("the image chose drivers from facter's report and kept NVIDIA's unloaded"):
          # QEMU's display is not an NVIDIA card, so no rule matches and the
          # NVIDIA modules the image carries stay out (tests/hardware.nix
          # covers a matching report).
          machine.succeed("systemctl is-active losos-hardware.service losos-hardware-fallback.service")
          machine.succeed("grep -q graphics_card /run/losos/hardware/facter.json")
          machine.succeed("grep -q '\"matched\": \\[\\]' /run/losos/hardware/plan.json")
          machine.succeed("modinfo -n nvidia")
          machine.fail("test -d /sys/module/nvidia")

      with subtest("a user is a homed LUKS volume"):
          # homed's first-boot wizard runs only while no regular user exists,
          # so nothing the image ships -- derisk-greeter included -- may count
          # as one. accounts.nix silences a NixOS warning on that reading.
          # userdbctl also prints "begin/end systemd-homed users" marker rows
          # for the UID ranges a filter covers, and homed's range is regular,
          # so without --boundaries=no the output is never empty.
          machine.fail(
              "userdbctl user --disposition=regular --no-legend --boundaries=no | grep -q ."
          )
          machine.succeed(
              "NEWPASSWORD=correct-horse homectl create alice --storage=luks "
              "--disk-size=256M --member-of=wheel --enforce-password-policy=no"
          )
          machine.succeed("userdbctl user alice | grep -q 'Storage: luks'")
          # glibc's NSS reaches nss-systemd through the configured nscd
          # forwarder, so homed users must be visible to ordinary lookups.
          machine.succeed("getent passwd alice")

      with subtest("sysupdate sees the installed version"):
          # The test VM has no route to the release feed, and without
          # --offline sysupdate fails on the download before listing what is
          # installed.
          machine.succeed(
              "${nodes.machine.systemd.package}/lib/systemd/systemd-sysupdate --offline list | grep -q '1'"
          )

      with subtest("pm is installed as the system manager"):
          machine.succeed("pm --help")
          machine.succeed("pm source-path https://example.org/x-1.0.tar.xz | grep -q x-1.0.tar.xz")

      with subtest("the terminal is derisk-terminal, with a Nerd Font, and foot is gone"):
          machine.succeed("command -v derisk-terminal")
          machine.succeed("test -e /run/current-system/sw/share/applications/org.derisk.terminal.desktop")
          machine.fail("command -v foot")
          machine.succeed("fc-list : family | grep -q 'Hack Nerd Font'")
          machine.succeed("fc-list : family | grep -q 'Symbols Nerd Font'")

      with subtest("the security report answers on the system bus"):
          machine.succeed(
              "busctl call io.losos.Security1 /io/losos/Security1 "
              "org.freedesktop.DBus.Introspectable Introspect"
          )

      with subtest("An update's GRUB replaces the one on the ESP"):
          # losos-grub-update runs once the bless has, and records what it
          # installed. A GRUB that is not this version's, and the
          # systemd-boot older installs started with, are both replaced,
          # and the reboot below starts the result.
          machine.wait_for_unit("losos-grub-update.service")
          machine.succeed(f"test -f {esp}/EFI/losos/installed")
          grub = "${nodes.machine.system.build.grub.efi}/grub.efi"
          machine.succeed(f"cmp {grub} {esp}/EFI/BOOT/BOOTX64.EFI")
          machine.succeed(f"printf older >> {esp}/EFI/BOOT/BOOTX64.EFI")
          machine.succeed(f"mkdir -p {esp}/EFI/systemd")
          machine.succeed(
              f"cp ${nodes.machine.systemd.package}/lib/systemd/boot/efi/systemd-bootx64.efi {esp}/EFI/systemd/"
          )
          machine.succeed(f"rm {esp}/EFI/losos/installed")
          machine.succeed("systemctl restart losos-grub-update.service")
          machine.succeed(f"cmp {grub} {esp}/EFI/BOOT/BOOTX64.EFI")
          machine.succeed(f"cmp {grub} {esp}/EFI/systemd/systemd-bootx64.efi")
          machine.succeed(f"test -f {esp}/EFI/losos/installed")

      with subtest("LosOS's firmware entry goes back to the front of the boot order"):
          # The entry the Windows installer adds, and Windows putting its
          # own back in front of it (grub.nix, losos-boot-order).
          efibootmgr = "${nodes.machine.system.build.grub.efibootmgr}/bin/efibootmgr"
          machine.wait_for_unit("losos-boot-order.service")
          part = machine.succeed(
              "lsblk --noheadings --output PARTN "
              f"$(findmnt --noheadings --output SOURCE --types vfat --mountpoint {esp})"
          ).strip()
          machine.succeed(
              f"{efibootmgr} --quiet --create --disk /dev/vda --part {part} "
              "--label LosOS --loader '\\EFI\\losos\\grubx64.efi'"
          )
          ours = machine.succeed(
              f"{efibootmgr} | sed -n 's/^Boot\\([0-9A-F]\\{{4\\}}\\)\\* LosOS\\([[:space:]].*\\)\\?$/\\1/p'"
          ).strip()
          order = machine.succeed(f"{efibootmgr} | sed -n 's/^BootOrder: //p'").strip().split(",")
          others = [entry for entry in order if entry != ours]
          assert others, order
          machine.succeed(f"{efibootmgr} --quiet --bootorder {','.join(others + [ours])}")
          machine.succeed("systemctl restart losos-boot-order.service")
          machine.succeed(f"{efibootmgr} | grep -qx 'BootOrder: {ours},.*'")

      with subtest("A counted UKI is blessed once it boots under UEFI GRUB"):
          # As boot-bios.nix checks it under a BIOS: the name sysupdate gives
          # a new UKI (update.nix), counted down in grubenv and settled by
          # losos-grub-bless.
          machine.succeed(
              f"mv {esp}/EFI/Linux/losos-desktop_1.efi '{esp}/EFI/Linux/losos-desktop_1+3-0.efi'"
          )
          machine.shutdown()
          machine.start()
          machine.wait_for_unit("multi-user.target")
          machine.wait_for_unit("losos-grub-bless.service")
          machine.succeed(f"test -f {esp}/EFI/Linux/losos-desktop_1.efi")
          machine.fail(f"ls {esp}/EFI/Linux/ | grep -q '+'")
          machine.fail(f"grep -q '^losos_' {esp}/EFI/losos/grubenv")
    '';
}
