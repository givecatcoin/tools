#!/bin/bash
# =============================================================================
# OMV 長期運用向け: OS ディスク以外の Btrfs を日次スナップショットする
#
# 保持: 年齢区間 ( -1,1 ], (1,2 ], (2,4 ], ... (128,256 ] で各最大 1 本
# 配置: 各マウント直下 .snapshots/daily-YYYYMMDD
# 想定: OMV の「スケジュールされたジョブ」から毎日 1 回呼び出す
#
# 例:
#   /usr/local/sbin/omv-btrfs-snap.sh
#
# テスト用（通常は不要）:
#   OMV_BTRFS_SNAP_BIN   モックコマンドディレクトリを PATH 先頭へ
#   OMV_BTRFS_SNAP_LOCK  ロックファイルパス
#   OMV_BTRFS_SNAP_TODAY 基準日 YYYYMMDD
# =============================================================================

# cron 向け既定 PATH。テスト時は OMV_BTRFS_SNAP_BIN にモックを置いて先頭へ足す
PATH="${OMV_BTRFS_SNAP_BIN:+$OMV_BTRFS_SNAP_BIN:}/usr/sbin:/usr/bin:/sbin:/bin"
export PATH

set -euo pipefail

SNAPDIR=".snapshots"
PREFIX="daily"
KEEP="1 2 4 8 16 32 64 128 256"
MAX_AGE=256
LOCK_FILE="${OMV_BTRFS_SNAP_LOCK:-/var/lock/omv-btrfs-snap.lock}"
LOG_TAG="omv-btrfs-snap"
TODAY="${OMV_BTRFS_SNAP_TODAY:-$(date +%Y%m%d)}"

# =============================================================================
# ログ
# =============================================================================
log() {
  logger -t "$LOG_TAG" -- "$*"
  printf '%s %s\n' "$(date -Iseconds)" "$*"
}

die() {
  log "ERROR: $*"
  exit 1
}

# =============================================================================
# 二重起動防止
# =============================================================================
exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  log "another run holds the lock; exit"
  exit 0
fi

trap 'die "failed at line $LINENO (exit $?)"' ERR

# =============================================================================
# 前提コマンド
# =============================================================================
for cmd in findmnt lsblk btrfs flock date basename mkdir; do
  command -v "$cmd" >/dev/null 2>&1 || die "command not found: $cmd"
done

# =============================================================================
# OS ディスク名（例: sda / nvme0n1）
# =============================================================================
ROOT_SRC="$(findmnt -n -o SOURCE / | sed 's/\[.*//')"
[ -n "$ROOT_SRC" ] || die "cannot resolve root SOURCE"
OS_DISK="$(lsblk -no PKNAME "$ROOT_SRC" 2>/dev/null | head -n 1 || true)"
if [ -z "$OS_DISK" ]; then
  OS_DISK="$(lsblk -no NAME "$ROOT_SRC" | head -n 1)"
fi
[ -n "$OS_DISK" ] || die "cannot resolve OS disk"
log "start TODAY=$TODAY OS_DISK=$OS_DISK"

# =============================================================================
# daily-YYYYMMDD の年齢（日）。失敗時は空を返す
# =============================================================================
snap_age_days() {
  local path="$1"
  local name day
  name="$(basename "$path")"
  day="${name#"$PREFIX"-}"
  case "$day" in
    [0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]) ;;
    *)
      return 1
      ;;
  esac
  # GNU date（Debian / OMV）
  echo $((($(date -d "$TODAY" +%s) - $(date -d "${day:0:4}-${day:4:2}-${day:6:2}" +%s)) / 86400))
}

delete_snap() {
  local path="$1"
  local why="$2"
  log "delete ($(basename "$path")): $why"
  btrfs subvolume delete "$path" >/dev/null
}

# =============================================================================
# 刈り込み: 区間ごとに最新 1 本だけ残す
# =============================================================================
prune() {
  local dir="$1"
  local path age t prev best best_age

  [ -d "$dir" ] || return 0

  # 古すぎるものを先に削除
  for path in "$dir"/"$PREFIX"-*; do
    [ -d "$path" ] || continue
    if ! age="$(snap_age_days "$path")"; then
      log "skip unparsable name: $(basename "$path")"
      continue
    fi
    if [ "$age" -gt "$MAX_AGE" ]; then
      delete_snap "$path" "age=$age > $MAX_AGE"
    fi
  done

  # 各区間 (prev, t] で age が最小（＝新しい）ものだけ残す
  prev=-1
  for t in $KEEP; do
    best=""
    best_age=""

    for path in "$dir"/"$PREFIX"-*; do
      [ -d "$path" ] || continue
      if ! age="$(snap_age_days "$path")"; then
        continue
      fi
      if [ "$age" -gt "$prev" ] && [ "$age" -le "$t" ]; then
        if [ -z "$best" ] || [ "$age" -lt "$best_age" ]; then
          best="$path"
          best_age="$age"
        fi
      fi
    done

    if [ -n "$best" ]; then
      for path in "$dir"/"$PREFIX"-*; do
        [ -d "$path" ] || continue
        if ! age="$(snap_age_days "$path")"; then
          continue
        fi
        if [ "$age" -gt "$prev" ] && [ "$age" -le "$t" ] && [ "$path" != "$best" ]; then
          delete_snap "$path" "bucket<=$t keep=$(basename "$best")"
        fi
      done
    fi

    prev=$t
  done
}

# =============================================================================
# 1 マウントを処理
# =============================================================================
process_mount() {
  local mnt="$1"
  local src disk dest

  src="$(findmnt -n -o SOURCE --target "$mnt" | sed 's/\[.*//')"
  disk="$(lsblk -no PKNAME "$src" 2>/dev/null | head -n 1 || true)"
  if [ -z "$disk" ]; then
    disk="$(lsblk -no NAME "$src" | head -n 1)"
  fi

  if [ "$disk" = "$OS_DISK" ]; then
    log "skip OS btrfs: $mnt (disk=$disk)"
    return 0
  fi

  # マウント点がサブボリュームであること（長期運用の前提）
  if ! btrfs subvolume show "$mnt" >/dev/null 2>&1; then
    log "skip not a subvolume mount: $mnt"
    return 0
  fi

  log "process: $mnt (disk=$disk)"

  # .snapshots は可能なら独立サブボリュームにする
  if [ ! -e "$mnt/$SNAPDIR" ]; then
    if ! btrfs subvolume create "$mnt/$SNAPDIR" >/dev/null 2>&1; then
      mkdir -p "$mnt/$SNAPDIR"
      log "note: $mnt/$SNAPDIR created as directory (not subvolume)"
    fi
  fi
  [ -d "$mnt/$SNAPDIR" ] || die "cannot create $mnt/$SNAPDIR"

  dest="$mnt/$SNAPDIR/$PREFIX-$TODAY"
  if [ -d "$dest" ]; then
    log "exists: $dest"
  else
    btrfs subvolume snapshot -r "$mnt" "$dest" >/dev/null
    log "created: $dest"
  fi

  prune "$mnt/$SNAPDIR"
}

# =============================================================================
# 本体
# =============================================================================
count=0
while IFS= read -r mnt; do
  [ -n "$mnt" ] || continue
  process_mount "$mnt"
  count=$((count + 1))
done < <(findmnt -nt btrfs -o TARGET | sort -u)

log "done mounts_seen=$count"
exit 0
