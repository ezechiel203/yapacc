#!/usr/bin/env bash
# Btrfs snapshot helper for yapacc (best-effort, host-specific).
# Focus: create/list/status of read-only snapshots for mounted btrfs subvols.
# Restore of a live root subvolume cannot be performed in-place safely; this
# script prints explicit rollback steps/commands for manual/boot-time recovery.
#
# This file is compiled into the yapacc binary (src/safety.rs) and run from
# there (`yapacc safety apply --snapshot-tag TAG` runs `create TAG`); yapacc
# never looks for it in the current directory.
set -euo pipefail

action="${1:-status}"
SNAP_BASE="${SNAP_BASE:-/.snapshots/yapacc}"
tag="${2:-manual}"

have() { command -v "$1" >/dev/null 2>&1; }

root_btrfs_info() {
  # prints: <source_device> <subvol_path> <mountpoint>
  local line src opts tgt sub
  line="$(findmnt -n -o SOURCE,OPTIONS,TARGET / 2>/dev/null || true)"
  [[ -z "$line" ]] && return 0
  src="$(awk '{print $1}' <<<"$line")"
  opts="$(awk '{print $2}' <<<"$line")"
  tgt="$(awk '{print $3}' <<<"$line")"
  sub="/"
  if [[ "$opts" =~ subvol=([^,]+) ]]; then sub="${BASH_REMATCH[1]}"; fi
  printf '%s %s %s\n' "$src" "$sub" "$tgt"
}

list_btrfs_mounts() {
  findmnt -rn -t btrfs -o SOURCE,TARGET,OPTIONS 2>/dev/null || true
}

status() {
  echo "== snapshot backend status =="
  have snapper && echo "snapper: present" || echo "snapper: absent"
  have timeshift && echo "timeshift: present" || echo "timeshift: absent"
  have btrfs && echo "btrfs: present" || echo "btrfs: absent"
  echo
  echo "== root filesystem =="
  local info; info="$(root_btrfs_info)"
  if [[ -n "$info" ]]; then
    echo "btrfs root: $info"
  else
    echo "btrfs root: not detected (or findmnt unavailable)"
  fi
  echo
  echo "== btrfs mounts =="
  list_btrfs_mounts || echo "(none)"
  echo
  echo "== existing yapacc snapshots =="
  if [[ -d "$SNAP_BASE" ]]; then
    find "$SNAP_BASE" -mindepth 2 -maxdepth 2 -type d 2>/dev/null | sort || true
  else
    echo "(none at $SNAP_BASE)"
  fi
}

# create read-only snapshots for selected mountpoints (default: /) on btrfs
create() {
  if ! have btrfs; then echo "error: btrfs command not found" >&2; exit 1; fi
  local ts; ts="$(date -u +%Y%m%dT%H%M%SZ)"
  local created=0
  # snapshot / if it is btrfs
  local src opts tgt mp sub
  while read -r src tgt opts; do
    [[ -z "$src" ]] && continue
    # default targets: root only (safe minimal). allow override via SNAP_TARGETS=" / /home "
    local targets="${SNAP_TARGETS:-/}"
    if [[ " $targets " != *" $tgt "* ]]; then continue; fi
    sub="/"
    if [[ "$opts" =~ subvol=([^,]+) ]]; then sub="${BASH_REMATCH[1]}"; fi
    local safe_name; safe_name="$(echo "$tgt" | sed 's#^/##; s#/#_#g; s/^$/root/')"
    local outdir="$SNAP_BASE/$ts/$safe_name"
    mkdir -p "$(dirname "$outdir")"
    echo "[snapshot] creating ro snapshot of $tgt (subvol=$sub) -> $outdir"
    if btrfs subvolume snapshot -r "$tgt" "$outdir"; then
      printf 'timestamp=%s\ntag=%s\nmountpoint=%s\nsource=%s\nsubvol=%s\n' "$ts" "$tag" "$tgt" "$src" "$sub" > "$outdir/.yapacc_snapshot"
      created=$((created+1))
    else
      echo "[snapshot] failed to snapshot $tgt (need privileges or unsupported layout)" >&2
    fi
  done < <(list_btrfs_mounts)
  if [[ $created -eq 0 ]]; then
    echo "error: no snapshots created. Ensure targets are btrfs mountpoints and run with required privileges." >&2
    exit 1
  fi
  echo "[snapshot] created $created snapshot(s) under $SNAP_BASE/$ts"
}

restore() {
  local snap_path="${2:-}"
  echo "== rollback guidance =="
  echo "Automatic in-place restore of a live root btrfs subvolume is unsafe and not performed."
  echo "To recover, boot into a rescue environment/live USB or another bootable snapshot/subvolume, then:"
  echo "  1) mount the btrfs top-level (subvolid=5 or subvol=/) of the device"
  echo "  2) rename/move the broken subvolume aside and replace it with a snapshot of the desired restore point"
  echo "     (or set default subvolume via: btrfs subvolume set-default <id> <mount>)"
  echo "  3) verify /etc/fstab, bootloader entries, and kernel/initramfs match the restored state"
  echo "  4) reboot"
  if [[ -n "$snap_path" ]]; then
    echo
    echo "Requested snapshot: $snap_path"
    echo "Example (manual, adjust device/mounts):"
    echo "  mount -o subvolid=5 /dev/<btrfs-device> /mnt"
    echo "  # inspect: btrfs subvolume list /mnt"
    echo "  # restore by replacing target subvol with snapshot: btrfs subvolume snapshot $snap_path /mnt/@root_restored  (or mv/swap as appropriate)"
  else
    echo
    echo "Usage for guidance with a specific snapshot: $0 restore <snapshot-path>"
  fi
  echo
  echo "Current snapshots:"; status >/dev/null 2>&1 || true
  if [[ -d "$SNAP_BASE" ]]; then find "$SNAP_BASE" -mindepth 2 -maxdepth 2 -type d 2>/dev/null | sort || true; fi
}

case "$action" in
  status) status ;;
  create) create ;;
  restore) restore "$@" ;;
  list) [[ -d "$SNAP_BASE" ]] && find "$SNAP_BASE" -mindepth 2 -maxdepth 2 -type d 2>/dev/null | sort || echo "(none)" ;;
  *) echo "usage: $0 {status|create [tag]|list|restore [snapshot-path]}" >&2; exit 2 ;;
esac
