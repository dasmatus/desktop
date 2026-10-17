# losos-flatpak-move-host <machine> <system|var-app|user-flatpak> <dir>:
# moves what Flatpak kept on the host, before it ran in a machine of its
# own, into the person's machine of that name (docs/hypervisor.md, "The
# Flatpak machine").
#
# <dir> becomes an ext4 disk, given to the machine read-only for one boot
# with the serial losos-flatpak-move; the guest's losos-flatpak-move copies
# it where <kind> says, through the guest agent; then the machine shuts down
# and the disk is gone. The machine must be off, since the disk is added to
# its boot rather than to a running machine: exit status 75 (EX_TEMPFAIL)
# says it was not, and to try again later.

machine=$1
kind=$2
source=$3

machines=${XDG_DATA_HOME:-$HOME/.local/share}/pm/machines
folder=$machines/$machine
# The unit pm's quickemu plugin and derisk start the machine as, so the
# desktop sees it running while it boots for this.
unit=pm-machine-$machine
serial=losos-flatpak-move
# Booting under emulation (no KVM) takes minutes; copying gigabytes, more.
boot_wait=1200
copy_wait=7200

# The agent's sync byte, 0xFF, is matched as a byte.
export LC_ALL=C

running() {
  systemctl --user --quiet is-active "$unit"
}

# One command to the guest agent on its socket in the machine's folder, its
# reply printed. Each call is a connection of its own, which starts with a
# sync whose answer is read past whatever an earlier call left unread, as
# the agent's protocol asks; a guest whose agent is not up yet answers
# nothing, and the call fails.
agent() {
  local id=$((RANDOM * 32768 + RANDOM)) line status=1
  coproc AGENT { socat - "UNIX-CONNECT:$folder/$machine-agent.sock" 2>/dev/null; }
  printf '\xff{"execute":"guest-sync-delimited","arguments":{"id":%d}}\n' "$id" >&"${AGENT[1]}"
  while IFS= read -r -t 5 line <&"${AGENT[0]}"; do
    line=${line##*$'\xff'}
    if [ "$(jq -r '.return? // empty' <<<"$line" 2>/dev/null)" = "$id" ]; then
      printf '%s\n' "$1" >&"${AGENT[1]}"
      if IFS= read -r -t 30 line <&"${AGENT[0]}"; then
        printf '%s\n' "$line"
        status=0
      fi
      break
    fi
  done
  kill "$AGENT_PID" 2>/dev/null || true
  wait "$AGENT_PID" 2>/dev/null || true
  return "$status"
}

if [ ! -e "$folder/machine.json" ] || [ ! -e "$machines/$machine.conf" ]; then
  echo "there is no machine $machine" >&2
  exit 1
fi
if running; then
  echo "$machine is running; its Flatpak files move once it is off" >&2
  exit 75
fi

work=$(mktemp -d --tmpdir=/var/tmp losos-flatpak-move.XXXXXX)
started=
cleanup() {
  if [ -n "$started" ] && running; then
    systemctl --user stop "$unit" || true
  fi
  rm -rf "$work"
}
trap cleanup EXIT

# Room for the files, their inodes (flatpak's repository is many small
# ones) and ext4's own; du counts a file with several links once, and so
# does mkfs. The guest finds the disk by its serial; the label, which ext4
# caps at 16 bytes, only names it.
bytes=$(du -sb "$source" | cut -f1)
files=$(find "$source" | wc -l)
size=$((bytes + bytes / 10 + 256 * 1024 * 1024))
truncate -s "$size" "$work/disk.img"
mkfs.ext4 -q -F -L flatpak-move -N $((files + files / 10 + 1024)) -d "$source" "$work/disk.img"

# The machine's own settings for this one boot, with the disk as a second
# virtio drive. quickemu names a machine after its settings file and finds
# its folder from its disk's path, so the copy, under the machine's name, is
# the machine: the same disk, firmware variables and sockets.
mkdir "$work/conf"
{
  cat "$machines/$machine.conf"
  printf "extra_args+=' -drive file=%s,if=none,id=flatpakmove,format=raw,readonly=on'\n" "$work/disk.img"
  printf "extra_args+=' -device virtio-blk-pci,drive=flatpakmove,serial=%s'\n" "$serial"
} >"$work/conf/$machine.conf"

started=1
systemd-run --user --unit="$unit" --description="The machine $machine" --collect --quiet \
  --property=Type=forking --property=PIDFile="$folder/$machine.pid" \
  quickemu --vm "$work/conf/$machine.conf"

deadline=$((SECONDS + boot_wait))
until agent '{"execute":"guest-ping"}' >/dev/null; do
  if [ "$SECONDS" -ge "$deadline" ] || ! running; then
    echo "$machine's guest agent did not answer" >&2
    exit 1
  fi
  sleep 2
done

pid=$(agent "$(jq -nc --arg kind "$kind" '{execute: "guest-exec", arguments: {
  path: "/run/current-system/sw/bin/losos-flatpak-move", arg: [$kind], "capture-output": true}}')" |
  jq -r '.return.pid')
deadline=$((SECONDS + copy_wait))
while :; do
  status=$(agent "{\"execute\":\"guest-exec-status\",\"arguments\":{\"pid\":$pid}}" || true)
  if [ "$(jq -r '.return.exited' <<<"$status" 2>/dev/null)" = true ]; then
    break
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "copying into $machine did not finish" >&2
    exit 1
  fi
  sleep 2
done
code=$(jq -r '.return.exitcode // 1' <<<"$status")
if [ "$code" != 0 ]; then
  jq -r '.return["err-data"] // empty' <<<"$status" | base64 -d >&2 || true
  echo "copying into $machine failed ($code)" >&2
  exit 1
fi

# Shut down as its power button would, through QEMU's monitor, so what it
# wrote is on its disk before the disk goes.
printf 'system_powerdown\n' | socat - "UNIX-CONNECT:$folder/$machine-monitor.socket" >/dev/null
deadline=$((SECONDS + boot_wait))
while running; do
  if [ "$SECONDS" -ge "$deadline" ]; then
    systemctl --user stop "$unit"
    break
  fi
  sleep 2
done
