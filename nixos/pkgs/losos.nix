# losos, the terminal client for LosOS boxes: the same panes as a box's admin
# pages, over a signed and encrypted envelope, with no SSH
# (docs/losos-boxes.md). It is written in the LosOS repository
# (github.com/dasmatus/losos, admin-ui/tui), which builds the same crate as
# its `.#losos`; this is that build, from a pinned commit.
#
# fetchgit with a sparse checkout rather than a components/ submodule: the
# LosOS tree is about 140 MB, handbook pictures and the admin UI included,
# which the flake would copy on every evaluation (docs/trust.md), and the
# client needs only these directories. Its build script reads the admin UI's
# catalogue, palette, pane keywords and the salmon, and the crate links the
# envelope crate in backend/access by path, which is why those are here.
{
  lib,
  rustPlatform,
  fetchgit,
  makeWrapper,
  libsecret,
  gnupg,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "losos";
  # The release tag nix/version.nix carried at this commit, which
  # `losos --version` and its About pane show.
  version = "0.4.0-unstable-2026-10-16";

  src = fetchgit {
    url = "https://github.com/dasmatus/losos";
    rev = "337acc9a66ccced01dd5d5fd3c5b85fa4573b896";
    sparseCheckout = [
      "admin-ui/tui"
      "admin-ui/app/src/i18n"
      "admin-ui/app/src/styles"
      "admin-ui/app/src/screens/settings"
      "admin-ui/themes/brand"
      "backend/access"
    ];
    hash = "sha256-rePTAM66QVyRYxXCqPYb34t8AoH5JgCpvuqydRc3kvc=";
  };

  cargoRoot = "admin-ui/tui";
  buildAndTestSubdir = "admin-ui/tui";
  cargoHash = "sha256-aaKB33odE4FKAVdFNMiXl2jzr25fkglKHT2bR4QZXSM=";

  env.LOSOS_VERSION = "v0.4.0";

  nativeBuildInputs = [ makeWrapper ];

  # Its suites start a mock box and edge over TLS and drive a pseudo
  # terminal; the LosOS repository's CI runs them on every change.
  doCheck = false;

  # It keeps keys and tokens in the Secret Service through libsecret's
  # secret-tool (gnome-keyring answers it, desktop.nix), and `losos key
  # import --gpg` exports the key from gpg. Suffixed, so a gpg the user
  # installed with pm comes first.
  postInstall = ''
    wrapProgram $out/bin/losos \
      --suffix PATH : ${
        lib.makeBinPath [
          libsecret
          gnupg
        ]
      }
  '';

  meta = {
    description = "Manage LosOS boxes and edges from a terminal, without SSH";
    homepage = "https://losos.dasmat.us/manual/terminal/";
    license = lib.licenses.agpl3Plus;
    mainProgram = "losos";
  };
})
