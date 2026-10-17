# What is not done

- **pm on a NixOS host.** pm's jail mirrors the host's `/bin`, `/lib` and
  `/usr`, and additionally mounts `/nix/store` and the store-backed `PATH`
  directories read-only, so a step whose tools come from the store runs: a
  `losos-nix` step (`nix eval`, `nix hash`) ran that way in pm's jail on a
  Nix-provisioned host. What still finds nothing is a recipe that names FHS
  paths, `/bin/cat` or a compiler under `/usr`, as every recipe in the old pm
  tree did; in this image `/usr` is the Nix store's partition and `/bin` holds
  only `sh`. So pm itself runs here (`pm --help`, `pm source-path`, signing,
  `explain`), and the boot test checks it is installed, but a recipe written
  for an FHS host does not build on it. For the same reason seven of pm's test targets
  are skipped in the nix build (the list is in `nixos/pkgs/pm.nix`).
- **The guest template in full** ([Virtual machine desktop](hypervisor.md)).
  Only a reduced build of it, without Danube, Flatpak and the Calculator,
  has booted, and no machine's window has reached a desktop through it yet.
  It is a whole second NixOS in `/usr`, so it carries its own copy of store
  paths the host has too (Mesa, Danube, fonts) and takes that room from the
  8 GiB `/usr` slot; giving machines the host's store over virtiofs would
  save it. A machine's disk grows into the size it was made with, once;
  nothing makes an existing machine's disk larger.
  Portal dialogs (a Flatpak's file chooser) in a machine are GTK's, drawn
  through the same waypipe, and have not been tried.
- **The Flatpak machine's loose ends**
  ([The Flatpak machine](hypervisor.md#the-flatpak-machine)). A
  `.flatpakref` or `.flatpakrepo` file opened on the host has nothing to
  open it, as Bazaar is in the machine; it has to be copied there first.
  Moving Flatpak's files off the host happens only for administrators, as
  anyone else would be asked for a password by a dialog they did not
  start; they copy their `~/.var/app` into the machine by hand. The host's
  copy of `~/.var/app` stays after it is copied, until its owner deletes
  it. Flatpak apps inside the machine draw in software, like the rest of
  its apps, so a game from Flathub is slower than it was on the host.
- **Windows and other distributions in machines**
  ([Machines from an installer CD](hypervisor.md#machines-from-an-installer-cd)).
  No Windows has been installed from the Windows tools CD yet, so its answer
  file, its drivers and its first sign-in have only been checked for what
  they say, not run; nor has the Linux tools CD's `setup.sh` run in any
  distribution. The desktop does not yet show a machine's own screen by
  itself (`virt-viewer --attach` does) or frame WinApps' windows in the
  machine's colour: FreeRDP's RemoteApp windows are X11's, drawn through
  the session's one xwayland-satellite, which belongs to no machine.
- **No hosted proxy.** GitHub Pages serves files only, so of what `proxy/`
  did ([Binary cache](binary-cache.md)), updates moved to GitHub Releases
  through an index on the documentation site and a service on each machine
  ([Releases](releases.md#updating-from-github-releases)), and CI reads the
  Nix cache through the proxy's own code on its runner. Three things still
  need a deployment that runs code, set as `LOSOS_PROXY_URL`: the daily
  `/ping` and the choice screens it turns on ([Choice
  screens](choice-screens.md)), stay off without one; the web flasher
  cannot download by itself and sends the person to the release page
  ([Web flasher](web-flasher.md#where-the-files-come-from)); and a builder
  outside CI has no public cache URL, though `proxy/serve.ts` runs the same
  cache locally. Systems installed from an image built with a proxy URL
  keep asking that proxy for updates until they are reinstalled or it
  serves them one more update.
- **A complete image build.** CI builds release images within its runner time
  budget; local image builds may take longer.
- **Hibernation.** No machine hibernates: the locked-down kernel refuses
  to, and nothing in mainline Linux authenticates a hibernation image that
  would let it ([Hibernation](hibernation.md)).

- **Signing.** There is no
  key rotation: a new key needs an update signed by the old one that carries
  both. Secure Boot signing
  of the UKI is not done either; as in the pm tree, a key belongs to whoever
  owns the machine.
- **The Windows installer has not run on Windows.** Its install was checked
  into a disk image laid out like Windows', which then booted
  ([The Windows installer](windows-installer.md)); the half that talks to
  Windows (shrinking C:, the partition table IOCTLs, the ESP mount, firmware
  variables, BitLocker) has only been compiled. Beyond that:
  - **Secure Boot must be off**, for the reason above.
  - **SmartScreen.** The `.exe` has no Authenticode signature, so Windows
    warns before running it.
  - **Windows' recovery partition.** Updates that grow it (as KB5034441
    did) shrink C: to make room behind it; with LosOS there they cannot,
    and fail as they do on any disk without free space after C:.
  - **The boot order.** Some firmware, and some Windows updates, put
    Windows Boot Manager first again. LosOS puts its own entry back in
    front each time it starts, but until it next does, Windows starts by
    itself and LosOS is one pick in the firmware's boot menu away.
  - **arm64.** The installer is built for x86_64 only, because Windows on
    arm64 runs on laptops this OS has no kernel configuration for.
- **The two gnome-control-center patches** in `nixos/pkgs/patches/`. Nothing
  upstream has them, so they stay. They are written against
  gnome-control-center 51.0, and nixpkgs carries 50.4, which moved the
  System panel to Blueprint; neither applies, and gnome-control-center is no
  longer in the image for them to go into. What they added is on derisk
  Settings' System page instead: the security report, and the factory reset,
  which only a UEFI machine offers, because systemd's request to reset
  needs one. Neither has been tried on hardware.
- **NVIDIA on real hardware.** The driver choice was tested in a VM with a
  report that names an RTX 4090 ([Hardware and NVIDIA](drivers.md)); no
  NVIDIA card has run the image. Untested: the open module taking a card,
  derisk's compositor on it, explicit sync, suspend,
  VA-API, hardened_malloc under NVIDIA's libraries, and laptops with two
  GPUs. A machine with an NVIDIA card and another GPU draws on whichever
  is the boot display; nothing offloads an app to the other.
- **derisk drives one display.** `derisk display-manager` starts
  `derisk greeter`, derisk's lock screen as the login screen, and after a
  login `derisk session --execute`; each takes the seat from logind and
  scans out through DRM/KMS on the first connected display, at its preferred
  mode (`nixos/modules/desktop.nix`). A second display stays dark and
  hotplug is not handled. X11 apps run through xwayland-satellite, a user
  unit that starts with the session (`desktop.nix`); it was seen serving an
  X11 client under a headless weston, not yet under derisk's compositor.
- **derisk's portal has gaps.** Screen sharing (xdg-desktop-portal-wlr)
  copies frames into shared memory, not dma-bufs, and a shared window is cut
  out of the screen, so anything covering it is shared too. The color and
  area pickers show a screenshot frozen across the screen; they were tried
  under X11, not yet in a derisk session, and neither was the lock screen's
  picture.
- **The Halium GSI has not run on a device.** It evaluates for aarch64, and
  an x86_64 build of the same boot path booted under QEMU (see the pull
  request that made it the one target). No arm64 build has been flashed, no
  bootloader has loaded its `init_boot`, and the web flasher has only talked
  to a simulated fastboot device. What only a phone can confirm: that a
  generic kernel's configuration has everything systemd and the desktop use
  (`CONFIG_USER_NS` for Flatpak's sandbox above all), that `vendor_boot`'s
  ramdisk and this one unpack together as intended, and that Halium's
  Android 14 system image starts each vendor's HALs. Beyond that:
  - **The display.** derisk drives the display itself through DRM/KMS. A
    device whose kernel has a DRM driver (msm, mediatek, panfrost) can show
    the desktop with Mesa; one that only has Android's hwcomposer cannot,
    because nothing here drives hwcomposer. That needs a hwcomposer backend
    in derisk or a compositor in front of it.
  - **Updates.** sysupdate is not wired up; the flasher writes userdata whole,
    so installing a newer release erases the phone. There is no verity on the
    root either, and the bootloader stays unlocked.
  - **Phones without `init_boot`.** The flasher writes their own boot image
    back with this ramdisk after theirs. Its repacking matches AOSP's
    `mkbootimg` byte for byte for header versions 0 to 4, and it was run
    against a simulated phone in fastbootd, but no real bootloader has
    booted such an image yet, and a phone whose ramdisk links `/etc` or
    `/lib` elsewhere would merge badly with this one.
  - **Kernels older than 5.10.** Phones that launched with Android 11 or
    earlier run 4.x or 5.4 kernels, which systemd does not support at all.
    The flasher reads the kernel's version and refuses them; supporting
    them would mean an older systemd or a kernel built per device.
  - **hardened_malloc.** The PC build preloads it into every process; the GSI
    does not. Its default configuration reserves 32 GiB per size class per
    arena and needs a 48-bit address space, and Android kernels are usually
    built with 39-bit virtual addresses. The GSI needs a hardened_malloc with
    a smaller `CONFIG_CLASS_REGION_SIZE`, and derisk built against it.
  - **Telephony, audio, sensors, camera.** No ofono, no PulseAudio/PipeWire
    droid modules, no sensorfw. Android's init starts the HALs, and nothing
    on the Linux side talks to them yet beyond EGL.
- **Wi-Fi, Bluetooth and power modes in Settings.** derisk Settings'
  Network page lists, joins and forgets networks through
  `wpa_supplicant`'s control socket, which only the `wpa_supplicant` group
  may use. First-boot setup puts the account it creates in that group; an
  account made before it did is not, and its Network page says so until
  `homectl update --member-of=wheel,wpa_supplicant <user>` adds it. The
  Bluetooth page pairs without asking anything, so a device that wants a
  passkey typed or compared does not pair. None of the three pages has run
  against a real adapter or `power-profiles-daemon` on hardware (the command
  palette's Power Mode rows ran against the daemon's placeholder driver on
  a private bus, which offers no Performance profile), and the
  Halium GSI has no Bluetooth: the phone's controller is behind its vendor
  HAL, which nothing here bridges to BlueZ.
- **Two LosOS disks in one PC.** Root comes from the disk the boot loader
  started from, under UEFI and a BIOS alike ([Boot loader](boot-loaders.md)),
  but swap is found by its partition label, `losos-swap`
  (`disk.nix`, `hibernate.nix`), so with two LosOS disks in one machine
  swap may be the other disk's.
- **sysext.** Extensions merge into `/usr`, which here holds little but the
  Nix store, so an extension can add a program and cannot replace one.
- **Android apps.** `nixos/modules/atl.nix` has two options, both off by
  default. `losos.android.enable` installs the Android Translation Layer.
  `losos.android.openApks` additionally makes it the default handler for
  `.apk` files. The package (`nixos/pkgs/android-translation-layer.nix`) is
  pinned to a full commit with a real fixed-output hash, and it evaluates, but
  it has not built to completion. Two gaps remain, and the package header
  lists the first: ATL's build shells out to the Android SDK build-tools
  (`dx`, `aapt`), which the recipe does not yet provide, and the pinned tree
  has no `thirdparty/art_standalone/build`, which ART's Makefile includes, so
  it needs another source input. The package is marked `meta.broken`, so
  enabling `losos.android` fails at evaluation with nixpkgs' broken-package
  error until both are fixed. Because both options are off, `nix flake
  check` never realises the package, so the bring-up can land and mature
  without gating the image.
  **APKs run in a sandbox that is not Android's.** Opening an `.apk`
  starts ATL through `losos-android-run`, under bubblewrap: the app sees the
  Nix store, `/etc` and the GPU read-only, a home of its own under
  `~/.local/share/losos/android/`, the Wayland and PipeWire sockets, the
  desktop portals through `xdg-dbus-proxy`, and the network, and none of
  the user's files, other sockets or session environment. That was checked
  with a stand-in for ATL, since ATL does not build yet. Android's per-app
  permissions are still not there: every app has the network, and through
  PipeWire the microphone, so `openApks`, which makes opening a file run
  it, stays a separate choice.
- **Booted under QEMU only.** Every CI run boots the x86_64 image in QEMU
  (`nix flake check`, [Boot loader](boot-loaders.md) "Tested"), under UEFI and
  under SeaBIOS: first boot lays out the disk with systemd-repart, root is
  found (by gpt-auto on UEFI, on the disk GRUB names on a BIOS), `/usr` is
  dm-verity, a new UKI is counted down and blessed, and a laptop with a TPM
  hibernates and resumes. The installer ISO starts under both firmwares, and
  an install from a nightly's files was run in QEMU through to the installed
  disk's first boot, so sysupdate has written a disk other than the one
  running. Not seen yet: a running system taking an update from the channel
  and booting it (the tests rename a UKI to stand in for a new one), the
  aarch64 image starting at all, since the arm64 runner has no KVM and the
  aarch64 VM under emulation times out waiting for its disk, and any of it
  on a PC rather than in QEMU.
- **Danube is unfinished in places** ([Danube](danube.md)). A page's file
  upload button opens nothing, since WebKit's file chooser request is not
  answered yet, and a site's notifications are never shown, even when
  allowed. Extensions cannot see requests (`webRequest`), talk to a
  native program or use cookies, context menus, history, bookmarks or the
  side panel ([Danube](danube.md), "Extensions"). There are no
  bookmarks, history list or find in page, and no way to resume a
  download. Its sandboxes need unprivileged user namespaces, and a phone's
  own kernel may refuse them; Danube then stops rather than running a
  page outside one, so on such a phone it does not open at all.
  `--no-sandbox` skips only the outer one. It has run against WPE WebKit
  in a session, not yet on a desktop or phone with derisk.
- **Ad blocking is DNS-wide only for names.** A filter list's URL and
  element rules apply in Danube alone; other browsers and apps get only
  whole blocked domains ([System-wide ad blocking](adblock.md)), and a
  program with its own DNS over HTTPS skips it entirely.
- **The LosOS boxes machine passes on names, not everything**
  ([LosOS boxes](losos-boxes.md)). It routes the other machines'
  connections to a box only on ports 80 and 443, by the name they ask for;
  an app's own port on a box is dialled directly, and nothing stops a
  machine that knows a box's address from reaching it without the LosOS
  boxes machine. Its keyring has no sign-in to unlock it, so the first
  `losos` that stores a key or token asks for a keyring password in a
  window of the machine. Nothing makes the machine by itself either: its
  owner makes it from the template. Neither half has met a real box on a real LAN
  yet, only a box that the machines test boots beside the host.
