# losos-guest-run: what the host's desktop asks of this guest, through the
# QEMU guest agent (derisk-machines' guest module holds the other half).
#
#   losos-guest-run --list   one line per app the desktop can open here:
#                            its desktop file ID, a tab, and its name
#   losos-guest-run <id>     start that app as the guest's user and return
#
# The agent runs this as root, with an environment of its own; the app runs
# in the user's systemd manager instead, with WAYLAND_DISPLAY pointing at the
# guest's waypipe server, so its windows go to the host's desktop.
#
# The same script runs in the LosOS guest template and, from the Linux tools
# CD, in other distributions (nixos/guest-tools/linux/); what differs between
# them is substituted in: the user, and the waypipe server's socket.

user=@user@
display=@display@

# Where desktop files are, the user's own first, as XDG orders them: an ID
# found earlier hides the same ID further down.
dirs=(
  "/home/$user/.local/share/applications"
  "/home/$user/.local/share/flatpak/exports/share/applications"
  /var/lib/flatpak/exports/share/applications
  /run/current-system/sw/share/applications
  /usr/local/share/applications
  /usr/share/applications
)

# The [Desktop Entry] group's value for `key`, unlocalised, from `file`.
entry() {
  awk -F= -v key="$2" '
    /^\[/ { group = $0; next }
    group == "[Desktop Entry]" && $1 == key { sub(/^[^=]*=/, ""); print; exit }
  ' "$1"
}

# Whether `file` is an app a person can open from the desktop: an
# application, not hidden, and not one that needs a terminal around it,
# which nothing on the host would give it.
openable() {
  [ "$(entry "$1" Type)" = Application ] &&
    [ "$(entry "$1" NoDisplay)" != true ] &&
    [ "$(entry "$1" Hidden)" != true ] &&
    [ "$(entry "$1" Terminal)" != true ] &&
    [ -n "$(entry "$1" Exec)" ]
}

if [ "${1-}" = --list ] && [ $# -eq 1 ]; then
  declare -A seen
  for dir in "${dirs[@]}"; do
    [ -d "$dir" ] || continue
    for file in "$dir"/*.desktop; do
      [ -e "$file" ] || continue
      id=$(basename "$file" .desktop)
      [ -z "${seen[$id]-}" ] || continue
      seen[$id]=1
      if openable "$file"; then
        printf '%s\t%s\n' "$id" "$(entry "$file" Name)"
      fi
    done
  done
  exit 0
fi

if [ $# -ne 1 ] || [ -z "$1" ] || [[ $1 == -* ]] || [[ $1 == */* ]]; then
  echo "usage: losos-guest-run --list | losos-guest-run <desktop file ID>" >&2
  exit 2
fi
id=${1%.desktop}

for dir in "${dirs[@]}"; do
  file="$dir/$id.desktop"
  [ -e "$file" ] || continue
  if ! openable "$file"; then
    echo "$id cannot be opened from the desktop" >&2
    exit 1
  fi
  # The command line without the desktop entry's field codes: nothing is
  # opened with it, so the files and URLs they stand for are empty, and
  # %i, %c and %k only ever add an icon, a name and a path.
  exec=$(entry "$file" Exec | sed -e 's/%[fFuUdDnNickvm]//g' -e 's/%%/%/g')
  # A login shell, so the app has the profile's PATH and XDG_DATA_DIRS,
  # which a unit of the user's manager does not; Exec= quotes the way the
  # shell does. The manager's own PATH has no bash, so it is named by the
  # path of the one running this. --collect, so a unit whose app failed
  # does not linger.
  exec systemd-run --quiet --collect --user --machine="$user@.host" \
    --description="$(entry "$file" Name)" \
    --setenv=WAYLAND_DISPLAY="$display" --setenv=XDG_SESSION_TYPE=wayland \
    -- "$BASH" -lc "exec $exec"
done

echo "no app $id here" >&2
exit 1
