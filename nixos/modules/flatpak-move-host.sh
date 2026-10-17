# losos-flatpak-move-host <machine> <system|var-app|user-flatpak> <dir>:
# moves what Flatpak kept on the host, before it ran in a machine of its
# own, into that machine (docs/hypervisor.md, "The Flatpak machine").
#
# <dir> becomes an ext4 disk in libvirt's default pool, given to the
# machine read-only for one boot with the serial losos-flatpak-move; the
# guest's losos-flatpak-move copies it where <kind> says, through the guest
# agent; then the machine shuts down and the disk is gone. The machine must
# be off, since a disk is added to its definition rather than to a running
# machine: exit status 75 (EX_TEMPFAIL) says it was not, and to try again
# later.

machine=$1
kind=$2
source=$3

serial=losos-flatpak-move
volume=$serial-$kind-$(id -u)
# Booting under emulation (no KVM) takes minutes; copying gigabytes, more.
boot_wait=1200
copy_wait=7200

agent() {
  virsh -q qemu-agent-command "$machine" "$1"
}

if [ "$(virsh -q domstate "$machine")" != "shut off" ]; then
  echo "$machine is running; its Flatpak files move once it is off" >&2
  exit 75
fi

work=$(mktemp -d --tmpdir=/var/tmp losos-flatpak-move.XXXXXX)
attached=
cleanup() {
  rm -rf "$work"
  if [ -n "$attached" ]; then
    if [ "$(virsh -q domstate "$machine")" != "shut off" ]; then
      virsh -q destroy "$machine" >/dev/null 2>&1 || true
    fi
    virsh -q detach-disk "$machine" vdb --config >/dev/null 2>&1 || true
  fi
  virsh -q vol-delete --pool default "$volume" >/dev/null 2>&1 || true
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

virsh -q vol-delete --pool default "$volume" >/dev/null 2>&1 || true
virsh -q vol-create-as default "$volume" "$size" --format raw
virsh -q vol-upload --pool default "$volume" "$work/disk.img"
rm -f "$work/disk.img"
path=$(virsh -q vol-path --pool default "$volume")

virsh -q attach-disk "$machine" "$path" vdb --config --mode readonly \
  --serial "$serial" --targetbus virtio
attached=1
virsh -q start "$machine"

deadline=$((SECONDS + boot_wait))
until agent '{"execute":"guest-ping"}' >/dev/null 2>&1; do
  if [ "$SECONDS" -ge "$deadline" ]; then
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
  status=$(agent "{\"execute\":\"guest-exec-status\",\"arguments\":{\"pid\":$pid}}")
  if [ "$(jq -r '.return.exited' <<<"$status")" = true ]; then
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

# Shut down as its power button would, so what it wrote is on its disk,
# before the disk is taken out of its definition.
virsh -q shutdown "$machine"
deadline=$((SECONDS + boot_wait))
until [ "$(virsh -q domstate "$machine")" = "shut off" ]; do
  if [ "$SECONDS" -ge "$deadline" ]; then
    virsh -q destroy "$machine"
    break
  fi
  sleep 2
done
