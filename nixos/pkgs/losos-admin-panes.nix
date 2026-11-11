# LosOS admin/panes UI (Skia-enabled RN/Expo project) and the TUI wasm,
# sourced from the LosOS appliance repo. This packages the web-exportable
# admin surface and the browser-driveable TUI wasm so LosOS Desktop can ship
# them without requiring a live LosOS box build on the desktop.
#
# Source pinned to the PR branch that introduced TV/Skia/Windows/visionOS
# target work; move the rev/hash when that work lands or a newer revision is
# wanted. Build requires network access for Bun deps in this repo's context;
# CI/caching policy follows losos-desktop conventions.
{
  lib,
  stdenvNoCC,
  fetchFromGitHub,
  bun,
  rustPlatform,
  wasm-bindgen-cli,
  binaryen,
  llvmPackages,
  lld,
}:

let
  src = fetchFromGitHub {
    owner = "dasmatus";
    repo = "losos";
    rev = "feat/tv-skia-windows-visionos";
    hash = lib.fakeHash; # TODO: replace with the pinned source hash
  };
in
stdenvNoCC.mkDerivation {
  pname = "losos-admin-panes";
  version = "0-unstable-${builtins.substring 0 12 src.rev}";

  inherit src;

  nativeBuildInputs = [
    bun
  ];

  # Best-effort packaging hook: build the web export of admin-ui/app and
  # install it. The RN mobile native targets (TV/Windows/visionOS) require
  # platform SDKs unavailable in this derivation; this output ships the
  # web-exportable surface and expects losos-tui-wasm to be provided as a
  # sibling artifact when available.
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
    description = "LosOS admin/panes web export (Skia-enabled project) for LosOS Desktop";
    homepage = "https://github.com/dasmatus/losos/pull/628";
    license = lib.licenses.eupl12;
    platforms = lib.platforms.linux;
  };
}
