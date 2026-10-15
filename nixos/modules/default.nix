# LosOS Desktop as a NixOS module, for UEFI PCs and arm64 machines. Importing
# this is the whole OS; a host configuration adds nothing but its architecture
# and version (see flake.nix).
#
# docs/nixos.md maps each file here to the piece of the pm tree it replaces.
{
  imports = [
    ./base.nix
    ./boot.nix
    ./grub.nix
    ./bios.nix
    ./splash.nix
    ./boot-sound.nix
    ./hardware.nix
    ./nvidia.nix
    ./libvirt.nix
    ./machines.nix
    ./allocator.nix
    ./image.nix
    ./disk.nix
    ./hibernate.nix
    ./update.nix
    ./installer.nix
    ./windows-installer.nix
    # testing.nix used to be here, with two units for tests to boot into.
    # losos-selftest printed a marker after boot-complete.target on a boot
    # whose command line said `losos.selftest`. No test ever booted with that
    # word, and the unit was still in every boot's transaction: multi-user.target
    # wanted it, it waited for boot-complete.target, and that waits for
    # systemd-boot-check-no-failures, which runs after multi-user.target. systemd
    # broke the cycle by dropping one job, sometimes boot-complete.target, and
    # then losos-grub-bless never marked the UKI on trial good, so an update
    # that booted fine could still be rolled back. The VM tests wait for the
    # bless itself (nixos/tests/boot.nix, boot-bios.nix).
    # losos-ota-test fetched a release over plain HTTP from 10.0.2.2 and
    # installed it with `systemd-sysupdate --verify=no`, on a word no test
    # ever passed either, so on real hardware it could only install an
    # unverified /usr and UKI for whoever controls the command line and that
    # address. A test that exercises sysupdate rewrites the transfers from the
    # test driver, not from a unit inside the image it is testing.
  ];
}
