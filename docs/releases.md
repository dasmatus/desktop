# Releases and GHCR

`nix build .#release` writes a flat directory:

```
losos-desktop_<v>_x86_64.efi                        the UKI sysupdate installs
losos-desktop_<v>_usr-x86-64_<uuid>.raw.xz          /usr for a sysupdate slot
losos-desktop_<v>_usr-x86-64-verity_<uuid>.raw.xz   its hash tree
losos-desktop_<v>.linux, .initrd, .grub             the UKI's kernel, initrd and
                                                    command line, for GRUB on a
                                                    BIOS PC, x86_64 only
losos-desktop_<v>_x86_64.raw.xz                     the disk image
losos-desktop_<v>_x86_64-installer.iso              the installer
losos-desktop_<v>_x86_64-windows-installer.exe      the installer for Windows, x86_64 only
SHA256SUMS
```

CI pushes it to GHCR as one OCI artifact per architecture with `oras`, and
`proxy/` serves the newest one to sysupdate, which cannot fetch from an OCI
registry itself (see [Binary cache](binary-cache.md)). The publish job then
creates a GitHub release of its own for that run, tagged
`nightly-<YYYYMMDD>T<HHMM>` from the version (`20261010.175322` is
`nightly-20261010T1753`) and marked a prerelease. It carries both
architectures' files, the update images, disk images, installer ISOs and the
[Windows installer](windows-installer.md). Each architecture's signed manifest
goes beside them as `SHA256SUMS-<arch>` and `SHA256SUMS-<arch>.gpg`.

GitHub rejects a release asset of 2 GiB or more, and `/usr` and the disk
image are past that, so such a file is uploaded in 2000 MiB parts,
`<file>.part-00` on; `cat <file>.part-* > <file>` gives back the file its line
of `SHA256SUMS-<arch>` names. That is also why sysupdate keeps reading
through the proxy rather than from the release.

The installer ISO a release carries installs from that same release: its
backend serves the release's files to sysupdate under the names a channel
has, joining split files as it streams them ([The installer](installer.md)).

A release is created once and never edited, so the repository can make
releases immutable. A re-run of the same commit finds its tag taken and
publishes nothing. There is no moving `nightly` release: GitHub refuses to
reuse the tag of a deleted immutable release, so it could not be replaced.

A tagged release is a run of `ci` started by hand (Actions, ci, Run
workflow) on the commit to release, with a **tag** such as `v0.1.0` and a
**title** such as `LosOS 0.1`. The run checks the tag first and fails within
a minute if it is malformed, starts with `nightly`, or is already released.
Otherwise it builds the commit for the **stable channel**
(`losos.channel = "stable"`, which `flake.nix` takes from `LOSOS_CHANNEL`
in CI), mostly from the cache, and publishes the same kinds of files as a
nightly would, as a full release rather than a prerelease. Its images
update from `<proxy>/updates/stable/<arch>/`, and the run moves
`images:stable-<arch>` in GHCR to it, so a system installed from a tagged
release, or from its installer ISO or Windows installer, follows the tagged
releases, and one installed from a nightly keeps following the nightlies.
Only what names the channel differs from the nightly of the same commit:
the update URL in `/usr`'s sysupdate transfers, and so `/usr`, the UKI and
the images built from them; every package is the same store path. A run
started by hand without a tag publishes a nightly.

![CI pushes store paths, releases and compiler caches to GHCR; the proxy redirects Nix, sysupdate and CI to GHCR's storage and streams the GSI's files to the web flasher; each nightly is also a GitHub release](images/binary-cache.svg)

The `/usr` halves are cut out of the finished disk image at the offsets repart
reported, not built a second time, so the bytes sysupdate installs are the
bytes the image boots. Each name carries its partition's UUID, which sysupdate
reads with `@u` and gives the partition it writes. The initrd finds `/usr` by
the UUIDs repart derived from `usrhash=`, so a slot that kept the random UUID
repart created it with would hold the right bytes and never be found.
