# Finds the LosOS boxes on the local network through systemd-resolved and
# offers each one as an application (nixos/modules/losos-boxes.nix,
# docs/losos-boxes.md).
#
# Built from src/losos-boxes. Cargo.lock pins every crate with its SHA-256,
# and importCargoLock turns each pin into a fixed-output fetch.
{
  lib,
  rustPlatform,
}:

rustPlatform.buildRustPackage {
  pname = "losos-boxes";
  version = "0.1.0";

  src = ../../src/losos-boxes;

  cargoLock.lockFile = ../../src/losos-boxes/Cargo.lock;

  # resolved's replies, the boxes' bookkeeping and the entries they get;
  # no network or resolved needed. nixos/tests/losos-boxes.nix finds real
  # boxes.
  doCheck = true;

  meta = {
    description = "Offer the LosOS boxes on the local network as applications";
    license = lib.licenses.agpl3Plus;
    platforms = lib.platforms.linux;
    mainProgram = "losos-boxes";
  };
}
