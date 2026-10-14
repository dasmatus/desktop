# Boot the real image on a legacy BIOS: SeaBIOS, GRUB from the MBR, the
# UKI's kernel and initrd (modules/bios.nix). boot.nix checks what the system
# does once it is up, which is the same under either firmware; this checks
# what is particular to a BIOS boot, and that GRUB's boot counting ends where
# systemd-boot's would.
{ self }:

{ lib, ... }:

{
  name = "losos-desktop-boot-bios";

  # The module adds its own overlay, which the test framework's shared,
  # read-only package set would refuse.
  node.pkgsReadOnly = false;

  nodes.machine =
    { lib, ... }:
    {
      imports = [ self.nixosModules.default ];

      losos.version = "1";

      # As in boot.nix: nobody is at the seat to answer first-boot setup.
      services.homed.promptOnFirstBoot = lib.mkForce false;
      systemd.services.derisk-setup.enable = false;

      # Started by the BIOS from the image's own disk, with no EFI firmware.
      virtualisation = {
        directBoot.enable = false;
        mountHostNixStore = false;
        useEFIBoot = false;
        fileSystems = lib.mkVMOverride { };
        memorySize = 3072;
      };

      boot.initrd.systemd.repart.device = "/dev/vda";
      boot.initrd.systemd.services.systemd-repart.after = lib.mkForce [ "dev-vda.device" ];
      swapDevices = lib.mkVMOverride [
        {
          device = "/dev/mapper/swap";
          options = [ "nofail" ];
        }
      ];
    };

  testScript =
    { nodes, ... }:
    ''
      import os
      import subprocess
      import tempfile

      tmp = tempfile.NamedTemporaryFile()
      subprocess.run([
        "${nodes.machine.virtualisation.qemu.package}/bin/qemu-img", "create",
        "-f", "qcow2", "-F", "raw",
        "-b", "${nodes.machine.system.build.disk}/${nodes.machine.image.filePath}",
        tmp.name, "32G",
      ], check=True)
      os.environ["NIX_DISK_IMAGE"] = tmp.name

      machine.wait_for_unit("multi-user.target")

      with subtest("GRUB started the system, and first boot found its disk"):
          machine.fail("test -d /sys/firmware/efi")
          machine.succeed("grep -q 'root=PARTLABEL=root-x86-64' /proc/cmdline")
          machine.succeed("grep -q 'usrhash=' /proc/cmdline")
          machine.succeed("findmnt --kernel --types ext4 /")
          machine.succeed("findmnt --kernel /home")
          assert "ACTIVE" in machine.succeed("dmsetup info --target verity usr")

      with subtest("The ESP is mounted without EFI to name it"):
          esp = machine.succeed("bootctl --print-esp-path").strip()
          machine.succeed(f"test -f {esp}/EFI/Linux/losos-desktop_1.efi")
          machine.succeed(f"test -f {esp}/EFI/Linux/losos-desktop_1.linux")
          machine.succeed(f"test -f {esp}/EFI/losos/grubenv")
          # The bless runs after boot-complete.target, which comes after
          # multi-user.target, so it can still be queued here; waiting
          # still fails if its job was dropped or it failed.
          machine.wait_for_unit("losos-grub-bless.service")

      with subtest("An update rewrites GRUB's BIOS boot code"):
          # Without the record of what it installed, losos-grub-update
          # writes the boot code again, so a BIOS boot partition emptied
          # here is whole again for the reboot below.
          machine.wait_for_unit("losos-grub-update.service")
          machine.succeed(f"test -f {esp}/EFI/losos/installed")
          bios = machine.succeed(
              "lsblk --raw --noheadings --output PATH,PARTTYPE /dev/vda"
              " | awk '$2 == \"21686148-6449-6e6f-744e-656564454649\" { print $1 }'"
          ).strip()
          machine.succeed(f"dd if=/dev/zero of={bios} bs=1M count=1 conv=fsync")
          machine.succeed(f"rm {esp}/EFI/losos/installed")
          machine.succeed("systemctl restart losos-grub-update.service")
          machine.fail(f"cmp --bytes=512 {bios} /dev/zero")

      with subtest("A counted UKI is blessed once it boots"):
          # The name sysupdate gives a new UKI (update.nix).
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
