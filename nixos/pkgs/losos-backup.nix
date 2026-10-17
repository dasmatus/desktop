# Each person's backups to a LosOS box: repositories to LosOS Git, the rest
# of the home to LosOS cloud (nixos/modules/backup.nix, docs/backup.md).
#
# Built from src/losos-backup. Cargo.lock pins every crate with its SHA-256,
# and importCargoLock turns each pin into a fixed-output fetch.
{
  lib,
  rustPlatform,
  git,
}:

rustPlatform.buildRustPackage {
  pname = "losos-backup";
  version = "0.1.0";

  src = ../../src/losos-backup;

  cargoLock.lockFile = ../../src/losos-backup/Cargo.lock;

  # The walk, the names, the paths and the box's hosts files, and a
  # snapshot of uncommitted work in a scratch repository, which needs git;
  # no box or network. nixos/tests/backup.nix backs up to a real
  # Nextcloud and Forgejo.
  doCheck = true;
  nativeCheckInputs = [ git ];

  meta = {
    description = "Back up a home to a LosOS box's LosOS cloud and LosOS Git";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
    mainProgram = "losos-backup";
  };
}
