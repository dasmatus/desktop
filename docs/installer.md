# The installer

The pm tree's installer was `systemd-sysinstall`, new in systemd v261.
nixpkgs 26.05 shipped 260.4, so the installer does the same job by hand with
the same tools. nixos-unstable now carries 261, so sysinstall is available;
the installer has not been moved onto it yet. It is its own small live system, `nixos/installer/`, on its own ISO,
and `nixos/modules/installer.nix` evaluates it from the OS's configuration.

> [!TIP]
> A machine that keeps Windows uses the [Windows installer](windows-installer.md)
> instead, which installs the same release beside it.

Under UEFI, the ISO's appended FAT partition holds GRUB as
`EFI/BOOT/BOOT<ARCH>.EFI`, which chainloads the installer's UKI from the ISO
9660 filesystem. On x86_64 a legacy BIOS starts it too, through GRUB, from a
disc or a stick. Either way GRUB's menu and the Plymouth splash after it look
as they do on an installed system, and say "LosOS Desktop installer"
([Boot loader and boot screen](boot-loaders.md)). Either way the initrd mounts the ISO by its volume label and the Nix store from a squashfs
on it. It runs NetworkManager with NixOS's defaults, as the installed system
does. NetworkManager runs DHCP on every wired port, built in or USB, as soon
as a cable is in, at boot or later, and prefers it over Wi-Fi when both are
up; a machine with a cable in is online with nothing asked. Otherwise derisk
scans for and joins a Wi-Fi network through NetworkManager with `nmcli`.
Of the firmware NixOS would add, only
`linux-firmware` is on the medium, because most Wi-Fi cards do not start
without it.

![The installer's Disk page, listing the one disk it may install to, and the Erase this disk? page, which names the disk once more behind an Erase and install button](images/installer-pages.png)

The installer is `derisk installer` on tty1, a logind session run as root,
with a root shell on tty2 for anything it does not cover. It draws the same
pages first-boot setup is made of, so it works with a mouse, a touchscreen or
a phone-sized screen. It starts `losos-installer serve` (`src/losos-installer`)
as its backend and talks to it in JSON lines on stdin and stdout: derisk owns
the screen and the network, the backend owns the disks. The Network page
shows each wired port as having no cable, getting an address, or online, and
is skipped when the machine is already online. Then the backend lists the
disks, leaving out the one the ISO booted from and refusing any disk it did
not list, and the installer names the disk, its size and the source once more
behind an "Erase and install" button before anything is touched. Then:

1. The disk's old partition table is wiped and the kernel told to forget
   its partitions, which it refuses, stopping the install before anything is
   written, while one of them is mounted. repart adds the new ESP to the
   kernel itself to format it, and could not while an old partition with
   the same number was still known.
2. `systemd-repart --empty=force` lays out the ESP, with GRUB and an empty
   `grubenv` copied in, on x86_64 a BIOS boot partition for GRUB, and
   both `/usr` slots at full size, labelled
   `_empty` (sysupdate refuses a disk with only one). These are `disk.nix`'s
   own definitions, so the disk is laid out the way the
   installed system expects to find it.
3. `systemd-sysupdate update` fills them with the GitHub release of the
   version the ISO was built as, through the same transfers as `update.nix`,
   aimed at the chosen disk instead of `auto` and at the new ESP, mounted
   under `/run/losos-installer`. With `losos.update.pubring` set, the installer
   checks `SHA256SUMS.gpg` against it as an update does. On x86_64,
   `losos-grub-bios-install` then writes GRUB into the MBR and the BIOS boot
   partition, so the disk starts on a legacy BIOS as well.
4. The machine reboots into the installed system, whose first boot creates
   root, `/home` and swap from `disk.nix`, the path an image written
   with `dd` takes.

![derisk installer talks JSON lines to losos-installer serve, which runs systemd-repart, then systemd-sysupdate from the channel or a LOSOS-RELEASE disk, then reboots; the disk ends with the ESP, BIOS boot and slot A written, slot B laid out empty, and root, home and swap left to the first boot](images/installer.svg)

So an install is an update into an empty slot, from the release the ISO came
with ([Releases](releases.md)): an ISO from `nightly-20261010T2127` installs
that nightly, and one from a tagged release installs that release. Neither
is a copy of the OS on the medium. sysupdate cannot read a GitHub release
as it is, since each architecture's manifest there is `SHA256SUMS-<arch>`
and a file of 2 GiB or more is split into `.part-NN` assets, so
`losos-installer` serves the release to it from `127.0.0.1`: `SHA256SUMS`
and its signature are the architecture's, and a split file is its parts
one after the other. sysupdate checks the signature and every hash itself,
as it does for the channel. The backend finds the release through GitHub's
API by the version in its file names, a tagged release before the nightly
of the same build. Where it finds none, such as an ISO built from a branch
or a local build, the install reads the channel and gets its newest
release, through the channel's index on the documentation site
([Releases](releases.md)) when the OS updates from GitHub Releases.

The files come down with [aria2](https://aria2.github.io/) before
sysupdate reads them (`src/losos-installer/src/stage.rs`): one file at a
time, each split over one connection per hardware thread, as
`aria2c -x $(nproc)` would, at most 16, which is as many as aria2 opens
to one server. GitHub's release storage caps each connection's speed, so
the image arrives that many times faster than over curl's one connection,
which is how it came before. A connection that breaks off is opened again
and goes on from the byte it stopped at. Only the manifest, its signature
and the files the transfers name are fetched, not the ISO, the disk image
or the other architecture's.

aria2 writes each connection's range where it belongs in the file, so a
file is whole only once every range is in and cannot be handed to
sysupdate as it arrives. It has to be kept somewhere first, and the live
system's memory cannot hold a `/usr` of over 3 GiB on every machine. The disk
being installed can: until the first boot creates root, `/home` and swap,
everything after the `/usr` slots is free. So repart adds an ext4
partition labelled `losos-download` there, the size of the files and some
room, aria2 fetches into it, and once sysupdate has written the slots the
installer wipes the partition and deletes it, so the first boot finds the
free space it expects. Updates on the installed system still stream
through curl (`update-source.nix`): its free space is not a partition to
spare. The installed system then updates from the channel as any other
does. The repository comes from `GITHUB_REPOSITORY` in CI
(`losos.release.repository`), so a fork's ISO reads the fork's releases. The image used to be its own installer, with a second
UKI that booted it into a tmpfs root and copied its own `/usr` with
`CopyBlocks=`; that is gone, and so is the `losos.install` condition it
needed in `disk.nix`. When nixpkgs reaches v261, the installer should be
measured against `systemd-sysinstall` again.

A machine that cannot reach the channel installs from a release disk
instead: any filesystem labelled `LOSOS-RELEASE` holding a release's
`SHA256SUMS`, `SHA256SUMS.gpg` and the `/usr`, verity and UKI files they
list. udev mounts it at `/run/losos/release` whenever it appears, and the
installer then turns the transfers' sources into local files. sysupdate
verifies signatures only on downloads, so the installer checks the release
first: `SHA256SUMS` against the signing key with `gpg`, as `systemd-pull`
does, then every file on the disk against its line there, refusing a file
not listed. The case it exists for is a VM in a network that re-signs TLS
with its own authority, where the channel's certificate never verifies; with
the disk attached the installer does not need the network at all: the
backend's `hello` names the disk as `release`, and derisk then skips its
Network page.
