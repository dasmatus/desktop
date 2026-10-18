# Users do not exist until someone makes one, and then a user IS a LUKS volume.
#
# There is no human user in users.users and no /etc/passwd entry for one. A
# person is a systemd-homed record: a signed identity plus an encrypted home
# image on the /home partition, created with homectl. The display manager
# and its PAM stack see them through NSS, answered by nss-systemd from
# systemd-userdbd. The reason is the update model: /etc can be reset
# (disk.nix) and a user who lived there would go with it, while a homed user
# travels with their own home area.
#
# This is also where NixOS closes a gap the pm tree never did. That tree
# shipped no PAM configuration at all -- Linux-PAM's tarball carries none and
# gdm installed none -- so docs/limits.md had to say that nothing in the image
# could authenticate anyone. NixOS generates the PAM stack, and with homed
# enabled it puts pam_systemd_home into all four management groups of every
# service, which is the condition for a login to actually open the home area
# rather than succeed and find it locked.
{ config, lib, ... }:

{
  services.homed = {
    enable = true;

    # The first user is made by first-boot setup (setup.nix), on the seat,
    # before any login screen is reachable. Without one a fresh install would
    # reach the login screen with nobody to log in as, because there is no
    # useradd here and no account in the image.

    settings.Home = {
      # LUKS, stated rather than inherited: it is the whole design that the
      # user's volume is the encrypted thing, while root is not.
      DefaultStorage = "luks";
      DefaultFileSystemType = "ext4";
    };
  };

  # homed's console wizard used to ask for that user here, with the group
  # prompt turned on so the first user could be put in wheel. setup.nix
  # replaced it with derisk's pages, which always make the first user an
  # administrator, and keeps the wizard only as a fallback for a machine
  # derisk cannot draw on.

  # The homed module turns on userdbd, which serves homed's records over
  # Varlink, and NixOS's nscd (nsncd, a stateless forwarder), through which
  # glibc finds nss-systemd in the store. Neither is restated here.

  # No root password and no root login. Administration is `pm system`, which
  # asks polkit, which asks a wheel member to authenticate -- as that member
  # (pm.nix).
  #
  # NixOS refuses a configuration in which neither root nor any wheel user in
  # users.users can log in, to stop people locking themselves out. Every
  # administrator here is a homed user the NixOS module system never sees, so
  # that check cannot be satisfied and is turned off rather than defeated with
  # a password nobody should have. The way back in when no administrator can
  # sign in is the recovery shell docs/troubleshooting.md describes, which
  # takes someone at the machine.
  users.allowNoPasswordLogin = true;

  # No sudo, no su, no run0. Each of them made the person typing root; pm
  # does the work as its own user instead, and only manages units and
  # updates: nothing a session can reach runs a command as root. run0 was here
  # until pm took its place, sudo before it as run0's alias. systemd still
  # carries the run0 binary, but polkit refuses to let anyone but pm and root
  # manage units (pm.nix), so it can no longer start anything. su is a setuid
  # wrapper NixOS's shadow module always adds; this turns it off on the host,
  # and the guests, which have no pm system service, keep it (userland.nix).
  security.sudo.enable = false;
  security.run0.enable = false;
  security.wrappers.su.enable = false;
}
