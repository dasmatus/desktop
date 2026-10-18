# The command-line tools a person types, in their Rust versions wherever one
# is a drop-in: uutils' coreutils and findutils, sudo-rs's su, and Sequoia's
# gpg. Imported by base.nix and by the machine template in nixos/guest/, so a
# shell on the host and a shell in a machine answer the same way.
#
# This is the system profile only, /run/current-system/sw. Units, NixOS's own
# scripts and the build still run GNU: their PATH is set from pkgs.coreutils
# and friends by NixOS and by the modules here, and their scripts were written
# against GNU's behaviour. The GNU packages stay in the closure for them.
{
  config,
  lib,
  pkgs,
  ...
}:

{
  # hiPrio puts each of these above NixOS's own core packages, which it adds
  # at a lower priority so that exactly this can be done. Where uutils has no
  # binary of a name, GNU's still answers: of coreutils that is chcon and
  # runcon, which only mean anything under SELinux.
  #
  # Not replaced, because the Rust version is not yet a drop-in (each run
  # side by side with GNU's on the same input):
  # - diffutils: uutils' diff has no -r, and no diff3 or sdiff.
  # - sed: uutils' sed ignores -z, \U and \b, and `sed 'n;d'` prints every
  #   line instead of every other one.
  # - procps: uutils' ps rejects `ps aux` and pidof finds only one process.
  # - util-linux, tar, acl and login: uutils has no release of
  #   them, only snapshots of work in progress.
  # - grep, gawk, less, gzip and the rest: nothing in Rust takes their options.
  environment.systemPackages =
    map lib.hiPrio [
      pkgs.uutils-coreutils-noprefix
      pkgs.uutils-findutils
    ]
    ++ [ pkgs.sequoia-sq ];

  # Sequoia's GnuPG chameleon is gpg, gpgv and gpgconf for git's commit
  # signing and anything else that runs `gpg`, reading and writing GnuPG's
  # own ~/.gnupg, with sq, Sequoia's own interface, beside it above. Secret
  # keys stay with GnuPG's gpg-agent, so the package the module installs and
  # starts the agent from is both: symlinkJoin keeps the first path's name
  # where two collide, so gpg, gpgv and gpgconf are the chameleon's and
  # gpg-agent and dirmngr GnuPG's. The chameleon's gpgconf is GnuPG's with
  # its gpg line pointed at the chameleon, so the agent unit's --reload works.
  #
  # The agent asks for a passphrase through pinentry, and the image had
  # none: signing on the booted image failed with "No pinentry". The gnome3
  # one prompts through gcr's dialog on the desktop and falls back to curses
  # in a terminal with no display, as in a machine.
  programs.gnupg = {
    package = pkgs.symlinkJoin {
      name = "gnupg-sequoia-chameleon";
      paths = [
        pkgs.sequoia-chameleon-gnupg
        pkgs.gnupg
      ];
    };
    agent = {
      enable = true;
      pinentryPackage = pkgs.pinentry-gnome3;
    };
  };

  # GnuPG makes ~/.gnupg on first use and the chameleon refuses to run
  # without it ("GnuPG's home directory doesn't exist"), so a new account's
  # first `gpg --quick-gen-key` failed. Each user manager makes it at start.
  systemd.user.tmpfiles.rules = [ "d %h/.gnupg 0700 - - -" ];

  # su is shadow's, a setuid C program; sudo-rs's su takes the same options
  # and runs through the same PAM service. Only the guests (nixos/guest/)
  # install it: the host has no su at all, and administers itself through pm
  # (accounts.nix). It also asks for su-l, for `su -`, which NixOS
  # defines only in its sudo-rs module, so it is defined here the way su is.
  security.shadow.su.package = pkgs.sudo-rs;
  security.pam.services.su-l = {
    inherit (config.security.pam.services.su) rootOK forwardXAuth logFailures;
  };
}
