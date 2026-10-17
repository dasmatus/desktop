# Sonne, the code editor: the project's fork of Zed, with its agent's
# built-in skills compiled in, the pm recipe language, and derisk's MCP
# bridge registered when `derisk` is on PATH (docs/sonne.md).
#
# Built the way nixpkgs builds zed-editor (pkgs/by-name/ze/zed-editor), which
# is the same tree a few releases back. The fork took out the collaboration
# crates, but its audio crate still runs microphone input through WebRTC's
# audio processing, so libwebrtc stays; the remote server, which Zed copies
# to a host it opens over SSH, is left out.
#
# Pinned by revision rather than as a components/ submodule, like
# android_translation_layer and for the same reason (docs/trust.md, "The
# components/ submodules"): it is an upstream fork, and its tree, which the
# flake would copy on every evaluation, is the size of Zed's.
{
  lib,
  stdenv,
  rustPlatform,
  fetchgit,
  cmake,
  pkg-config,
  protobuf,
  cargo-about,
  makeBinaryWrapper,
  envsubst,
  fontconfig,
  libgit2,
  openssl,
  sqlite,
  zlib,
  zstd,
  glib,
  alsa-lib,
  libxkbcommon,
  wayland,
  libxcb,
  libx11,
  libxext,
  libxcursor,
  libxi,
  libxrandr,
  libGL,
  vulkan-loader,
  livekit-libwebrtc,
  nodejs,
  bubblewrap,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "sonne";
  version = "1.24.0-unstable-2026-10-16";

  # fetchgit rather than fetchFromGitHub, as android_translation_layer
  # does: a git clone reaches github.com from hosts that cannot use its
  # archive endpoint.
  src = fetchgit {
    url = "https://github.com/losos-project/Sonne";
    # main after Sonne#11.
    rev = "d0d974f1265dfa88b4ff1af750a67e04cbcb66c6";
    hash = "sha256-xGH3T7QBfOobWrEWRNd6zW0+9DJb4gIC3D9XPOFXrZw=";
  };

  cargoHash = "sha256-9NqA/E7Wvr0OoMMmDip/gVIO7sb07lLT7Jfc5PUycxk=";

  # Upstream's .cargo/config.toml asks for lld on aarch64, which would
  # fight mold (nixos/pkgs/default.nix, callWithMold), and for v0 symbol
  # mangling, which only makes backtraces longer. nixpkgs removes it too.
  # generate-licenses insists on the cargo-about version Zed's CI pins;
  # nixpkgs' newer one writes the same file.
  #
  # webrtc-sys links nixpkgs' prebuilt libwebrtc as a shared library rather
  # than downloading LiveKit's static one, and looks for glib's headers in
  # a sysroot there is none of.
  postPatch = ''
    rm .cargo/config.toml
    substituteInPlace $cargoDepsCopy/*/webrtc-sys-*/build.rs \
      --replace-fail "cargo:rustc-link-lib=static=webrtc" "cargo:rustc-link-lib=dylib=webrtc" \
      --replace-fail 'builder.include(&glib_path);' 'builder.include("${lib.getInclude glib}/include/glib-2.0");' \
      --replace-fail 'builder.include(&glib_path_config);' 'builder.include("${lib.getLib glib}/lib/glib-2.0/include");'
    substituteInPlace script/generate-licenses \
      --replace-fail '$CARGO_ABOUT_VERSION' '${cargo-about.version}'

    # Without these generate-licenses fails, and with it the build: mcsapi's
    # crates are GPL-3.0-only and egui's bundled fonts are under OFL-1.1 and
    # the Ubuntu font licence, none of which Zed's accepted list names.
    # Sonne is GPL-3.0-or-later and links them, so they are accepted for
    # those crates alone. This belongs in Sonne's own list.
    cat >> script/licenses/zed-licenses.toml <<'EOF'
    [mcsapi]
    accepted = ["GPL-3.0-only"]
    [mcsapi-components]
    accepted = ["GPL-3.0-only"]
    [mcsapi-ui]
    accepted = ["GPL-3.0-only"]
    [epaint_default_fonts]
    accepted = ["OFL-1.1", "Ubuntu-font-1.0"]
    EOF
  '';

  nativeBuildInputs = [
    cmake
    pkg-config
    protobuf
    cargo-about
    makeBinaryWrapper
  ];
  # cmake is for crates' build scripts, not for this tree.
  dontUseCmakeConfigure = true;

  buildInputs = [
    libgit2
    sqlite
    zlib
    zstd
    fontconfig
    openssl
    glib
    alsa-lib
    libxkbcommon
    wayland
    libxcb
    libx11
    libxext
  ];

  # The editor and its `sonne` launcher, which hands files to a running
  # editor or starts one; Sonne's agent window (sonne_desktop) is a
  # separate program and not shipped.
  cargoBuildFlags = [
    "--package=zed"
    "--package=cli"
  ];

  # Sonne's tests are its own repository's business; here they would build
  # the whole workspace a second time with test-support features.
  doCheck = false;

  env = {
    # Upstream's release profile is fat LTO in one codegen unit, a single
    # link that takes most of an hour and more memory than CI's 16 GB
    # runner has beside the other prebuilds (--max-jobs 1 there notwithstanding).
    # Thin LTO over 16 units links in parallel at a few percent of speed.
    CARGO_PROFILE_RELEASE_LTO = "thin";
    CARGO_PROFILE_RELEASE_CODEGEN_UNITS = "16";
    # Stripped in fixup anyway.
    CARGO_PROFILE_RELEASE_DEBUG = "false";
    ALLOW_MISSING_LICENSES = true;
    OPENSSL_NO_VENDOR = true;
    LIBGIT2_NO_VENDOR = true;
    LIBSQLITE3_SYS_USE_PKG_CONFIG = true;
    ZSTD_SYS_USE_PKG_CONFIG = true;
    # Turns the auto-updater off and says why: the image updates as a whole
    # (docs/releases.md).
    ZED_UPDATE_EXPLANATION = "Sonne is part of LosOS Desktop and updates with the system.";
    RELEASE_VERSION = finalAttrs.version;
    LK_CUSTOM_WEBRTC = livekit-libwebrtc;
  };

  # assets/licenses.md, which the editor's "Licenses" page shows.
  preBuild = ''
    bash script/generate-licenses
  '';

  # The launcher looks for the editor at ../libexec/zed-editor beside
  # itself (crates/cli, "possible_locations"). The desktop entry's file
  # name and icon are Sonne's Wayland app ID (crates/release_channel), so
  # derisk matches its windows to them.
  installPhase = ''
    runHook preInstall

    release_target="target/${stdenv.hostPlatform.rust.cargoShortTarget}/release"
    install -Dm755 $release_target/zed $out/libexec/zed-editor
    install -Dm755 $release_target/cli $out/bin/sonne

    install -Dm644 crates/zed/resources/app-icon.png $out/share/icons/hicolor/512x512/apps/io.github.dasmatus.Sonne.png
    install -Dm644 crates/zed/resources/app-icon@2x.png $out/share/icons/hicolor/512x512@2/apps/io.github.dasmatus.Sonne.png

    mkdir -p $out/share/applications
    DO_STARTUP_NOTIFY=true APP_CLI=sonne APP_ICON=io.github.dasmatus.Sonne APP_NAME=Sonne APP_ARGS=%U \
      ${lib.getExe envsubst} < crates/zed/resources/zed.desktop.in \
      > $out/share/applications/io.github.dasmatus.Sonne.desktop

    runHook postInstall
  '';

  # GPUI and mcsapi's window, which Sonne draws in by default (gpui_platform's
  # mcsapi-default), dlopen everything they draw with: libwayland-client and
  # libxkbcommon, libEGL and Vulkan's loader (the drivers come from
  # hardware.graphics), and under X11 the libraries x11rb looks up by name.
  # Language servers it installs itself run on node, which it would
  # otherwise download as a binary NixOS cannot run, and its agent's
  # terminal sandbox is bubblewrap.
  postFixup = ''
    patchelf $out/libexec/zed-editor --add-rpath ${
      lib.makeLibraryPath [
        wayland
        libxkbcommon
        libGL
        vulkan-loader
        libxcb
        libx11
        libxcursor
        libxi
        libxrandr
      ]
    }
    wrapProgram $out/libexec/zed-editor --suffix PATH : ${
      lib.makeBinPath [
        nodejs
        bubblewrap
      ]
    }
  '';

  meta = {
    description = "Code editor with an agent window, LosOS Desktop's fork of Zed";
    homepage = "https://github.com/losos-project/Sonne";
    license = lib.licenses.gpl3Plus;
    platforms = lib.platforms.linux;
    mainProgram = "sonne";
  };
})
