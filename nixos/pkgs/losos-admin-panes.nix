# LosOS admin/panes web export (Expo web), from the LosOS appliance repo.
# Nothing in nixos/modules installs this yet; the overlay entry is for
# follow-up wiring and `nix build .#losos-admin-panes`.
#
# Pinned to losos #628 (TV/Skia/Windows/visionOS admin UI); sparse-checkout
# admin-ui/app only, as the full tree is far larger than a web export needs
# (docs/trust.md, losos.nix).
{
  lib,
  stdenvNoCC,
  fetchgit,
  bun,
}:

let
  lososRev = "af65c6a0f8c78a38d445d8729bf5a1e40807d9d1";
  src = fetchgit {
    url = "https://github.com/dasmatus/losos";
    rev = lososRev;
    hash = "sha256-feumfi/Sy8x2oRqrTLkmQsyXuAkyNhhhInQJvCoCEf0=";
    sparseCheckout = [ "admin-ui/app" ];
  };
in
stdenvNoCC.mkDerivation {
  pname = "losos-admin-panes";
  version = "0-unstable-${builtins.substring 0 12 lososRev}";

  inherit src;

  nativeBuildInputs = [ bun ];

  buildPhase = ''
    runHook preBuild
    export HOME=$TMPDIR
    cd admin-ui/app
    bun install --frozen-lockfile --ignore-scripts || bun install --ignore-scripts
    bun run build:web
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    mkdir -p $out/share/losos-admin-panes
    cp -r admin-ui/app/dist/* $out/share/losos-admin-panes/
    runHook postInstall
  '';

  meta = {
    description = "LosOS admin/panes web export for LosOS Desktop";
    homepage = "https://github.com/dasmatus/losos/pull/628";
    license = lib.licenses.eupl12;
    platforms = lib.platforms.linux;
  };
}
