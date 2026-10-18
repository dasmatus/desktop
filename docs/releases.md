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

CI pushes it to GHCR as one OCI artifact per architecture with `oras`, which
a deployment of `proxy/` can serve to sysupdate, which cannot fetch from an
OCI registry itself (see [Binary cache](binary-cache.md)). The publish job then
creates a GitHub release of its own for that run, tagged
`nightly-<YYYYMMDD>T<HHMM>` from the version (`20261010.175322` is
`nightly-20261010T1753`) and marked a prerelease. It carries both
architectures' files, the update images, disk images, installer ISOs and the
[Windows installer](windows-installer.md). Each architecture's signed manifest
goes beside them as `SHA256SUMS-<arch>` and `SHA256SUMS-<arch>.gpg`.

GitHub rejects a release asset of 2 GiB or more, and `/usr` and the disk
image are past that, so such a file is uploaded in 2000 MiB parts,
`<file>.part-00` on; `cat <file>.part-* > <file>` gives back the file its line
of `SHA256SUMS-<arch>` names. sysupdate cannot read that as a channel, so
an image reads the release through a service of its own that joins the
parts as it streams them (below).

The installer ISO a release carries installs from that same release: its
backend fetches the release's files with aria2 onto the disk being
installed and serves them to sysupdate under the names a channel has,
joining split files ([The installer](installer.md)).

> [!NOTE]
> A release is created once and never edited, so the repository can make
> releases immutable. A re-run of the same commit finds its tag taken and
> publishes nothing. There is no moving `nightly` release: GitHub refuses to
> reuse the tag of a deleted immutable release, so it could not be replaced.

A tagged release is a run of `ci` started by hand (Actions, ci, Run
workflow) on the commit to release, with a **tag** such as `v0.1.0` and a
**title** such as `LosOS 0.1`. The run checks the tag first and fails within
a minute if it is malformed, starts with `nightly`, or is already released.
Otherwise it builds the commit for the **stable channel**
(`losos.channel = "stable"`, which `flake.nix` takes from `LOSOS_CHANNEL`
in CI), mostly from the cache, and publishes the same kinds of files as a
nightly would, as a full release rather than a prerelease. Its images
update from the stable channel's newest release (or a proxy's
`/updates/stable/<arch>/`), and the run moves `images:stable-<arch>` in
GHCR to it, so a system installed from a tagged
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

## The channel index on the site

The documentation site on GitHub Pages carries each channel's newest release
as a file, `updates/<channel>/<arch>/index`, written when the site is built
(`website/release-index.js`). It has one line per file sysupdate would ask
a channel for: the name, its size, and the release asset URLs it is made of,
so `SHA256SUMS` is the line for `SHA256SUMS-<arch>`, and a split file lists
its parts in order. The nightly channel's newest release is the newest
`nightly-*` prerelease; the stable channel's is the newest full release.

```
SHA256SUMS 1096 https://github.com/<repo>/releases/download/nightly-20261016T1200/SHA256SUMS-x86_64
losos-desktop_<v>_usr-x86-64_<uuid>.raw.xz 2621440000 https://...raw.xz.part-00 https://...raw.xz.part-01
```

`docs.yml` builds the site when the docs change, and the publish job starts
it once a release is up, so the index moves to a release only after the
release has every file. Pages serves files and nothing else, which is why
the index lists URLs rather than redirecting to them. Nothing in it is
trusted: whatever reads it still checks `SHA256SUMS.gpg` and each file's
hash.

## Updating from GitHub Releases

An image built without `LOSOS_PROXY_URL` updates from its channel's newest
GitHub release. Its sysupdate transfers read `http://127.0.0.1:8485/`
(`losos.update.baseUrl`), where a socket of its own,
`losos-update-source.socket` (`nixos/modules/update-source.nix`), starts a
short script for each request: it reads the channel's index above from
`losos.update.index`, finds the name asked for, and streams the release
assets it lists one after the other with curl. A connection that breaks off
is picked up from the byte it stopped at, up to ten times a part. sysupdate
gets `SHA256SUMS`, its signature and whole files under one URL, as from any
channel, and checks the signature against the image's key
(`/etc/systemd/import-pubring.pgp`, with gpg on the update unit's PATH) and
every file against its line. No service outside GitHub is involved.

A download that fails anyway leaves its slot marked partial, which sysupdate
will not install over. The update unit vacuums such slots before it starts,
so the next timer run downloads that version again.

The installer ISO reads the same index itself, for an install from the
channel when the backend finds no release of the ISO's own version, and
fetches the parts it lists with aria2 as it does a release's
([The installer](installer.md)). The [Windows installer](windows-installer.md)
reads the same index itself, downloads each part with resume and joins them
before it checks the file.

The index is read again for each request, so a release published in the
middle of an update can move it between `SHA256SUMS` and the files that name.
Every file name carries its version, so the files of the older release are
then not found, that attempt fails, and the next timer run starts over from
the new manifest.

> [!NOTE]
> Systems installed from an image that names a proxy keep reading that proxy:
> the URL is part of `/usr`, so only an update from it, or a reinstall, moves
> them to GitHub Releases.
