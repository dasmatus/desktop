# Boot the installer from its actual ISO under UEFI, or with `bios` under a
# legacy BIOS through GRUB's El Torito image (iso.nix), including the initrd
# mounts and the service that owns tty1.
{
  self,
  bios ? false,
}:

let
  # The test driver talks to a guest only through the backdoor shell that
  # nixpkgs' test-instrumentation.nix starts on hvc0. A test node gets that
  # module for free, but the ISO is a system of its own, evaluated in
  # installer.nix, so without it every machine.succeed waits for a shell that
  # never answers until the driver's global timeout kills the run. The module
  # is added to that system here, not to the medium that ships.
  installerSystem =
    self.nixosConfigurations."losos-desktop-x86_64".config.system.build.installerSystem.extendModules
      {
        modules = [
          (
            { modulesPath, ... }:
            {
              imports = [ "${modulesPath}/testing/test-instrumentation.nix" ];
            }
          )
        ];
      };
  installerIso = installerSystem.config.system.build.isoImage;

  # The installer's own command line, `derisk installer -- <backend> serve
  # ...`, shell-quoted by escapeShellArgs, with derisk's three words shifted
  # off so the backend runs alone and takes one request on stdin.
  install = disk: ''
    printf '%s\n' '{"method":"install","disk":"${disk}"}' \
      | (set -- ${installerSystem.config.systemd.services.derisk-installer.serviceConfig.ExecStart}; shift 3; "$@") \
      > /tmp/install.log 2>&1
  '';
in
{
  name = "losos-desktop-installer-boot${if bios then "-bios" else ""}";

  nodes.machine =
    { ... }:
    {
      virtualisation = {
        directBoot.enable = false;
        mountHostNixStore = false;
        useEFIBoot = !bios;
        qemu.options = [
          "-cdrom ${installerIso}"
          "-boot order=d"
        ];
        memorySize = 3072;
        # A disk to install onto, as large as repart's layout needs.
        emptyDiskImages = [ 20480 ];
      };
    };

  testScript = ''
    machine.${if bios then "fail" else "succeed"}("test -d /sys/firmware/efi")
    machine.succeed("findmnt --kernel /iso")
    machine.succeed("findmnt --kernel /nix/store")
    machine.wait_for_unit("derisk-installer.service")
    machine.succeed(
        "systemctl show derisk-installer.service --property=TTYPath --value "
        "| grep -qx /dev/tty1"
    )

    # A disk that already has a partition table, as any used disk does and
    # as a failed install leaves one. repart's ESP format used to fail here
    # with "Failed to create new partition '/dev/vdb2': Device or resource
    # busy", since the kernel still had the old partition 2. With nowhere to
    # download from, the install then stops at the download, and that it got
    # there is what is checked.
    machine.succeed("printf 'label: gpt\\n,1M\\n,512M\\n,128M\\n' | sfdisk /dev/vdb")
    machine.succeed("test -b /dev/vdb2")
    machine.execute(${builtins.toJSON (install "/dev/vdb")}, timeout=900)
    print(machine.succeed("cat /tmp/install.log"))
    machine.succeed("grep -q '\"label\":\"Downloading and writing the system\"' /tmp/install.log")
    machine.fail("grep -q 'resource busy' /tmp/install.log")
  '';
}
