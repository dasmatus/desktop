# Windows app support

Windows app support opens Windows apps as windows of their own on the
desktop. Each person who opens it gets a Windows of their own, which sets
itself up the first time and asks nothing. It is
[WinBoat](https://github.com/TibixDev/winboat) (MIT), under the name the
desktop gives it, with three changes of this OS's. x86-64 only, and it
needs KVM.

## What runs where

| | |
| --- | --- |
| Windows | Windows 11, in [dockur/windows](https://github.com/dockur/windows)' QEMU, inside a container |
| The container | the person's own, in rootless Podman, its disk in `~/.local/share/winboat` |
| Each app's window | a FreeRDP RemoteApp session, H.264 in full colour (AVC444) |
| Who starts the container | pm (`pm container`, its podman plugin), never Podman or Docker directly |

dockur downloads Windows from Microsoft, in the person's language, and
installs it unattended with an answer file. When Microsoft's servers turn
the download down, dockur tries mirrors of the same ISO. At Windows' first sign-in it runs WinBoat's guest server, a
service that lists Windows' apps and their icons for the app, and imports
the registry changes that let every program be a RemoteApp. The person's
home is shared with Windows as a network drive.

## The first time

The app grid's **Windows app support** runs `windows-app-support`
(`nixos/pkgs/windows-app-support.nix`). For a person with no Windows yet it
starts their user unit `windows-app-support-setup`, which does what
WinBoat's setup wizard does once it has its answers, with answers of its
own:

- the Windows account is the person's user name, as far as Windows takes
  one (letters and digits, at most 20), with a random password nobody
  types: WinBoat reads it back from the compose file for every session;
- the guest server checks the host by that password's Argon2id hash, the
  way WinBoat's wizard writes it;
- half the PC's memory (4 to 16 GiB) and processors (2 to 8), and a
  64 GiB disk;
- Windows in the person's language when dockur has it, English otherwise.

It writes `~/.winboat/podman-compose.yml` and WinBoat's settings, then has
pm bring the container up. WinBoat opens as soon as the container exists
and shows Windows installing; the setup carries on while it is closed, and
a notification says when Windows is ready, which takes most of an hour.
Since a container exists, WinBoat's own wizard never shows.

Windows is not activated: it runs as Windows does before a product key is
entered, and a key can be entered in Windows' own Settings.

## Rootless only

A container of root's would run with the whole system's privilege, so
there are none:

- WinBoat is patched (`nixos/pkgs/patches/winboat/`) to start, stop, list
  and inspect its container through `pm container`, and to offer Podman
  only. A setting that still says Docker gets Podman too.
- pm's podman plugin turns each request into Podman's commands, or
  podman-compose's for a compose file, and refuses root's requests
  ([pm](pm.md)).
- The image's `podman` refuses root outside a user namespace, and root's
  Podman API socket, which NixOS would open to the `podman` group, is not
  started (`nixos/modules/windows-apps.nix`). Docker is not installed.

pm runs only plugins its user signed. So that nobody has to, WinBoat and
the setup run a `pm` of their own that keeps its configuration under
`~/.local/state/windows-app-support`: its own key, and the image's podman
plugin signed with it, renewed when an update brings another. The
containers are the person's as ever, in their own Podman store, and
`podman ps` in a terminal shows them.

Rootless Podman maps a container's users onto the person's subordinate
IDs. People here are systemd-homed's, made after the image was built, so
`/etc/subuid` and `/etc/subgid` give each of homed's UIDs 65536 IDs of its
own by number, which shadow and Podman both look up.

## Windows' apps

WinBoat lists Windows' apps through its guest server and opens each as a
FreeRDP RemoteApp window. Each comes over RDP's graphics pipeline as H.264
in full colour: the image's WinBoat passes FreeRDP `/gfx:AVC444`, and the
registry changes it imports at the first sign-in set the policy that makes
Windows prefer it. Windows encodes it in software, as the container's QEMU
has no GPU; the host decodes it with FFmpeg.

## What was here before

WinApps did this, against a Windows a person installed themselves into a
machine from the Windows tools CD and then described to WinApps by hand:
its address, account and password. WinBoat installs the Windows it
serves, so nothing is described. A Windows machine made with `pm machine
create --system windows` still works, with its screen in virt-viewer
([Windows](hypervisor.md#windows)); it just has no app windows.
