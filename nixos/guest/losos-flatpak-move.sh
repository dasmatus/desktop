# losos-flatpak-move <system|var-app|user-flatpak>: copies Flatpak's files
# from a host whose Flatpak ran on the host itself, before Flatpak moved into
# this machine (docs/hypervisor.md, "The Flatpak machine"). The host's half,
# losos-flatpak-move-host, attaches them as a read-only disk with the serial
# losos-flatpak-move and runs this through the guest agent, as root:
#
#   system        the host's /var/lib/flatpak, into this machine's
#   var-app       a host account's ~/.var/app, the apps' own data, into
#                 the user's
#   user-flatpak  a host account's ~/.local/share/flatpak, the apps it
#                 installed for itself, into the user's
#
# What is already here stays where the disk has nothing of the same name.

user=@user@
disk=/dev/disk/by-id/virtio-losos-flatpak-move

case "${1-}" in
system) target=/var/lib/flatpak ;;
var-app) target=/home/$user/.var/app ;;
user-flatpak) target=/home/$user/.local/share/flatpak ;;
*)
  echo "usage: losos-flatpak-move system|var-app|user-flatpak" >&2
  exit 2
  ;;
esac

# The disk appears a moment after the machine starts with it.
udevadm settle
if [ ! -b "$disk" ]; then
  echo "no disk to move Flatpak's files from" >&2
  exit 1
fi

source=$(mktemp -d)
mount -o ro,nodev,nosuid,noexec "$disk" "$source"
trap 'umount "$source"; rmdir "$source"' EXIT

mkdir -p "$target"
# -a keeps the hard links flatpak's repository and checkouts share; one cp
# for all of it, so links between its parts are kept too.
mapfile -t parts < <(find "$source" -mindepth 1 -maxdepth 1 ! -name lost+found)
if [ "${#parts[@]}" -gt 0 ]; then
  cp -a "${parts[@]}" "$target/"
fi
if [ "$1" != system ]; then
  chown -R "$user:users" "/home/$user/.var" "/home/$user/.local"
fi
# On the disk before the host shuts the machine down and takes the source
# away.
sync
