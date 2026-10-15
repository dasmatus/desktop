# WinApps: a Windows machine's apps as windows of their own on the desktop,
# each a FreeRDP RemoteApp session to the machine (docs/hypervisor.md,
# "Windows"). nixpkgs does not carry it; this is upstream's own package
# (packages/winapps in its repository) at a pinned commit, with upstream's
# patch that points its setup at the store instead of a git clone.
#
# The Windows half of it, the registry changes and firewall rule that let a
# machine serve RemoteApps, is on the Windows tools CD (machines.nix), and
# runs at a new machine's first sign-in.
{
  lib,
  stdenv,
  fetchgit,
  makeWrapper,
  freerdp,
  dialog,
  libnotify,
  netcat,
  iproute2,
  libvirt,
}:

stdenv.mkDerivation (finalAttrs: {
  pname = "winapps";
  version = "0-unstable-2026-09-14";

  # fetchgit, not fetchFromGitHub: the hash was checked against a git
  # checkout, which applies the repository's .gitattributes (CRLF batch files,
  # UTF-16 registry files) the way the tarball may not.
  src = fetchgit {
    url = "https://github.com/winapps-org/winapps";
    rev = "42c7e8318280c6fc3426c7afebfc7f43b895f4c8";
    hash = "sha256-eSUFY+u9Xq9/i/0/OH5JhVzTvG+eiOEvUErAmfBzaBE=";
  };

  nativeBuildInputs = [ makeWrapper ];

  patches = [ "${finalAttrs.src}/packages/winapps/setup.patch" ];

  postPatch = ''
    substituteAllInPlace bin/winapps
    substituteAllInPlace setup.sh
    patchShebangs install/inquirer.sh
  '';

  # What upstream's package installs, less what only its docker and podman
  # flavours use: on LosOS the machine is libvirt's, which is where pm made
  # it, so `virsh` is on its PATH too.
  installPhase = ''
    runHook preInstall

    mkdir -p $out/src
    cp -r apps install bin oem LICENSE.md COPYRIGHT.md setup.sh $out/src/
    install -m755 -D bin/winapps $out/bin/winapps
    install -m755 -D setup.sh $out/bin/winapps-setup
    for program in winapps winapps-setup; do
      wrapProgram $out/bin/$program \
        --prefix PATH : ${
          lib.makeBinPath [
            freerdp
            dialog
            libnotify
            netcat
            iproute2
            libvirt
          ]
        }
    done

    runHook postInstall
  '';

  meta = {
    description = "Windows apps from a Windows machine, as windows of their own over RDP";
    homepage = "https://github.com/winapps-org/winapps";
    license = lib.licenses.agpl3Plus;
    mainProgram = "winapps";
    platforms = lib.platforms.linux;
  };
})
