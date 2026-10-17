# losos-guest-flatpak: what the host's Settings asks of the Flatpak machine
# through the QEMU guest agent (derisk-machines' flatpak module holds the
# other half). The agent runs this as root.
#
#   losos-guest-flatpak apps
#       `global <TAB> <overrides>`, then a line per installed app: `app`,
#       its ID, `user` or `system`, its name, and its metadata and the
#       user's overrides of it, both base64, separated by tabs
#   losos-guest-flatpak overrides <app>
#       standard input becomes the user's overrides of <app>; empty input
#       removes them
#   losos-guest-flatpak flatpak <permission-show|permission-set|permission-remove> ...
#       that `flatpak` command, as the user, in its session, where the
#       portals' permission store is

user=@user@
flatpak=@flatpak@
home=/home/$user
userdir=$home/.local/share/flatpak
sysdir=/var/lib/flatpak

usage() {
  echo "usage: losos-guest-flatpak apps | overrides <app> | flatpak permission-show|permission-set|permission-remove ..." >&2
  exit 2
}

# An app ID as flatpak takes one, and so a file name that stays in the
# overrides directory.
valid() {
  [[ $1 =~ ^[A-Za-z0-9_][A-Za-z0-9_.-]*$ ]] && [ "${#1}" -le 255 ]
}

# A file, base64-encoded on one line; nothing for a file that is not there.
encoded() {
  if [ -f "$1" ]; then
    base64 -w0 <"$1"
  fi
}

case "${1-}" in
apps)
  [ $# -eq 1 ] || usage
  printf 'global\t%s\n' "$(encoded "$userdir/overrides/global")"
  # The user's installation first, as flatpak runs the user's copy of an
  # app installed both ways.
  declare -A seen
  for kind in user system; do
    root=$sysdir
    [ "$kind" = system ] || root=$userdir
    for app in "$root"/app/*; do
      id=$(basename "$app")
      active=$app/current/active
      if ! valid "$id" || [ -n "${seen[$id]-}" ] || [ ! -f "$active/metadata" ]; then
        continue
      fi
      seen[$id]=1
      # A tab in the name would end its field early.
      name=$(sed -n 's/^Name=//p' "$active/export/share/applications/$id.desktop" 2>/dev/null |
        head -n 1 | tr -d '\t')
      printf 'app\t%s\t%s\t%s\t%s\t%s\n' "$id" "$kind" "$name" \
        "$(encoded "$active/metadata")" "$(encoded "$userdir/overrides/$id")"
    done
  done
  ;;
overrides)
  if [ $# -ne 2 ] || ! valid "$2"; then
    usage
  fi
  text=$(cat)
  # As the user, in the user's own directory: root writing there could be
  # led through a link the user's apps left.
  if [ -z "${text//[[:space:]]/}" ]; then
    exec runuser -u "$user" -- rm -f "$userdir/overrides/$2"
  fi
  runuser -u "$user" -- mkdir -p "$userdir/overrides"
  printf '%s\n' "$text" | runuser -u "$user" -- tee "$userdir/overrides/$2.new" >/dev/null
  exec runuser -u "$user" -- mv -f "$userdir/overrides/$2.new" "$userdir/overrides/$2"
  ;;
flatpak)
  shift
  case "${1-}" in
  permission-show | permission-set | permission-remove) ;;
  *) usage ;;
  esac
  # The permission store is a service of the user's session bus, which the
  # user's manager (lingering, common.nix) has from boot.
  exec systemd-run --quiet --user --machine="$user@.host" --pipe --wait --collect \
    --setenv=DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$(id -u "$user")/bus" \
    -- "$flatpak" "$@"
  ;;
*)
  usage
  ;;
esac
