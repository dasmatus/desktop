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
  environment.systemPackages = map lib.hiPrio [
    pkgs.uutils-coreutils-noprefix
    pkgs.uutils-findutils

    # Sequoia's GnuPG chameleon is gpg, gpgv and gpgconf for git's commit
    # signing and anything else that runs `gpg`, reading and writing GnuPG's
    # own ~/.gnupg. Secret keys stay with GnuPG's gpg-agent, which the
    # wrapper finds by itself. sq is Sequoia's own interface beside it.
    pkgs.sequoia-chameleon-gnupg
    pkgs.sequoia-sq
  ];

  # sudo is already Rust: run0's sudo shim (accounts.nix). su is shadow's, a
  # setuid C program; sudo-rs's su takes the same options and runs through
  # the same PAM service. It also asks for su-l, for `su -`, which NixOS
  # defines only in its sudo-rs module, so it is defined here the way su is.
  security.shadow.su.package = pkgs.sudo-rs;
  security.pam.services.su-l = {
    inherit (config.security.pam.services.su) rootOK forwardXAuth logFailures;
  };
}
