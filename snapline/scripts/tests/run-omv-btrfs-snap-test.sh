#!/bin/bash
# =============================================================================
# OMV 想定モック環境で omv-btrfs-snap.sh を検証する
# =============================================================================
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SCRIPT="$ROOT/omv-btrfs-snap.sh"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/omv-btrfs-snap-test.XXXXXX")"
BIN="$WORK/bin"
FS="$WORK/fs"
STATE="$WORK/state"
LOG="$WORK/test.log"

mkdir -p "$BIN" "$FS" "$STATE"
: >"$LOG"

pass=0
fail=0

# =============================================================================
# ヘルパ
# =============================================================================
ok() {
  pass=$((pass + 1))
  printf 'PASS  %s\n' "$1" | tee -a "$LOG"
}

ng() {
  fail=$((fail + 1))
  printf 'FAIL  %s\n' "$1" | tee -a "$LOG"
  if [ "${2:-}" != "" ]; then
    printf '      %s\n' "$2" | tee -a "$LOG"
  fi
}

assert_dir() {
  local msg="$1" path="$2"
  if [ -d "$path" ]; then
    ok "$msg"
  else
    ng "$msg" "missing dir: $path"
  fi
}

assert_no_dir() {
  local msg="$1" path="$2"
  if [ ! -d "$path" ]; then
    ok "$msg"
  else
    ng "$msg" "unexpected dir: $path"
  fi
}

assert_log_has() {
  local msg="$1" needle="$2" file="$3"
  if grep -Fq "$needle" "$file"; then
    ok "$msg"
  else
    ng "$msg" "not found in log: $needle"
  fi
}

cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

# =============================================================================
# モックコマンド（OMV/Debian 風の応答）
# =============================================================================
cat >"$BIN/logger" <<'EOF'
#!/bin/bash
# logger -t TAG -- message
shift || true
echo "logger: $*" >>"${OMV_TEST_STATE}/syslog"
EOF

cat >"$BIN/flock" <<'EOF'
#!/bin/bash
# flock -n FD  → 成功（実 flock が無い環境向け簡易モック）
# 本物の flock があればそれを使う
if command -v /usr/bin/flock >/dev/null 2>&1; then
  exec /usr/bin/flock "$@"
fi
if command -v /bin/flock >/dev/null 2>&1; then
  exec /bin/flock "$@"
fi
# 最終手段: -n と FD だけ受けて常に成功
exit 0
EOF

cat >"$BIN/findmnt" <<'EOF'
#!/bin/bash
# findmnt の必要最低限を再現
state="${OMV_TEST_STATE}"

if [ "${1:-}" = "-nt" ] && [ "${2:-}" = "btrfs" ]; then
  # findmnt -nt btrfs -o TARGET
  printf '%s\n' "/"
  printf '%s\n' "/srv/dev-disk-by-uuid-aaaa"
  printf '%s\n' "/srv/dev-disk-by-uuid-bbbb"
  exit 0
fi

if [ "${1:-}" = "-n" ] && [ "${2:-}" = "-o" ] && [ "${3:-}" = "SOURCE" ]; then
  shift 3
  target="/"
  if [ "${1:-}" = "/" ]; then
    target="/"
  elif [ "${1:-}" = "--target" ]; then
    target="${2:-}"
  fi
  case "$target" in
    /) echo "/dev/sda1" ;;
    /srv/dev-disk-by-uuid-aaaa) echo "/dev/sdb1" ;;
    /srv/dev-disk-by-uuid-bbbb) echo "/dev/sdc1" ;;
    *) exit 1 ;;
  esac
  exit 0
fi

exit 1
EOF

cat >"$BIN/lsblk" <<'EOF'
#!/bin/bash
# lsblk -no PKNAME|NAME DEV
opt=""
dev=""
while [ $# -gt 0 ]; do
  case "$1" in
    -no)
      opt="$2"
      shift 2
      ;;
    -n)
      shift
      ;;
    -o)
      opt="$2"
      shift 2
      ;;
    *)
      dev="$1"
      shift
      ;;
  esac
done

case "$opt" in
  PKNAME)
    case "$dev" in
      /dev/sda1) echo "sda" ;;
      /dev/sdb1) echo "sdb" ;;
      /dev/sdc1) echo "sdc" ;;
      *) exit 0 ;;
    esac
    ;;
  NAME)
    case "$dev" in
      /dev/sda1) echo "sda1" ;;
      /dev/sdb1) echo "sdb1" ;;
      /dev/sdc1) echo "sdc1" ;;
      *) exit 1 ;;
    esac
    ;;
  *)
    exit 1
    ;;
esac
EOF

cat >"$BIN/btrfs" <<'EOF'
#!/bin/bash
# btrfs のファイルシステム操作をディレクトリで模擬する
fsroot="${OMV_TEST_FS}"
state="${OMV_TEST_STATE}"

mark_subvol() {
  mkdir -p "$1"
  : >"$1/.is_subvol"
}

is_subvol() {
  [ -f "$1/.is_subvol" ]
}

cmd="${1:-}"
sub="${2:-}"
shift 2 || true

case "$cmd $sub" in
  "subvolume show")
    target="${1:-}"
    if is_subvol "$target"; then
      echo "Name:        $(basename "$target")"
      exit 0
    fi
    exit 1
    ;;
  "subvolume create")
    target="${1:-}"
    mark_subvol "$target"
    echo "Create subvolume '${target}'" >>"$state/btrfs.log"
    exit 0
    ;;
  "subvolume snapshot")
    # btrfs subvolume snapshot -r SRC DEST
    ro=""
    if [ "${1:-}" = "-r" ]; then
      ro=1
      shift
    fi
    src="${1:-}"
    dest="${2:-}"
    [ -d "$src" ] || exit 1
    mark_subvol "$dest"
    # 実データの浅いコピー印
    echo "snapshot from=$src to=$dest ro=$ro" >"$dest/.snap_meta"
    echo "snapshot $src -> $dest" >>"$state/btrfs.log"
    exit 0
    ;;
  "subvolume delete")
    target="${1:-}"
    [ -d "$target" ] || exit 1
    rm -rf "$target"
    echo "delete $target" >>"$state/btrfs.log"
    exit 0
    ;;
  *)
    echo "btrfs mock: unsupported: $cmd $sub $*" >&2
    exit 1
    ;;
esac
EOF

chmod +x "$BIN"/*

# date / basename / mkdir / head / sed / sort / grep は実コマンドを使う
# BIN を先頭にし、実コマンドは後ろの PATH から解決させる

# =============================================================================
# フェイク FS（OMV 風）
# =============================================================================
mkdir -p \
  "$FS/" \
  "$FS/srv/dev-disk-by-uuid-aaaa/data" \
  "$FS/srv/dev-disk-by-uuid-bbbb/media"

# ルートとデータディスクをサブボリューム扱い
: >"$FS/.is_subvol"
: >"$FS/srv/dev-disk-by-uuid-aaaa/.is_subvol"
: >"$FS/srv/dev-disk-by-uuid-bbbb/.is_subvol"

# findmnt が返すパスと一致させるため、スクリプト実行時の cwd ではなく
# 絶対パスでマウント点を見せる必要がある。
# モック findmnt は固定パスを返すので、それを FS 配下へバインドする代わりに
# findmnt を WORK 配下パスを返すよう書き換える。

cat >"$BIN/findmnt" <<EOF
#!/bin/bash
FS="$FS"

if [ "\${1:-}" = "-nt" ] && [ "\${2:-}" = "btrfs" ]; then
  printf '%s\\n' "\$FS"
  printf '%s\\n' "\$FS/srv/dev-disk-by-uuid-aaaa"
  printf '%s\\n' "\$FS/srv/dev-disk-by-uuid-bbbb"
  exit 0
fi

if [ "\${1:-}" = "-n" ] && [ "\${2:-}" = "-o" ] && [ "\${3:-}" = "SOURCE" ]; then
  shift 3
  target="/"
  if [ "\${1:-}" = "--target" ]; then
    target="\${2:-}"
  elif [ -n "\${1:-}" ]; then
    target="\$1"
  fi
  case "\$target" in
    "\$FS"|"/") echo "/dev/sda1" ;;
    "\$FS/srv/dev-disk-by-uuid-aaaa") echo "/dev/sdb1" ;;
    "\$FS/srv/dev-disk-by-uuid-bbbb") echo "/dev/sdc1" ;;
    *)
      # ルート SOURCE 問い合わせ
      if [ "\$target" = "/" ]; then
        echo "/dev/sda1"
        exit 0
      fi
      exit 1
      ;;
  esac
  exit 0
fi
exit 1
EOF
chmod +x "$BIN/findmnt"

# ルート判定用: スクリプトは findmnt / を呼ぶ。モックは / → sda1。
# データは FS 配下。OS スキップは「マウントの disk == OS_DISK」なので、
# ルート FS マウント（\$FS）も sda になりスキップされる想定。

# =============================================================================
# 既存スナップショットを仕込む（刈り込み検証）
# =============================================================================
TODAY_FIXED="20260721"
AAAA="$FS/srv/dev-disk-by-uuid-aaaa"
BBBB="$FS/srv/dev-disk-by-uuid-bbbb"
SNAP_A="$AAAA/.snapshots"
mkdir -p "$SNAP_A"

# age: 0=today created by script, seed older ones
# 区間内に複数あるケース
seed_snap() {
  local dir="$1" day="$2"
  mkdir -p "$dir/daily-$day"
  : >"$dir/daily-$day/.is_subvol"
  echo "seed $day" >"$dir/daily-$day/.snap_meta"
}

# bucket <=1 : age 0 (今日は実行時), age 1
seed_snap "$SNAP_A" "20260720" # age 1
seed_snap "$SNAP_A" "20260719" # age 2 → bucket 2
seed_snap "$SNAP_A" "20260718" # age 3 → bucket 4 と競合用にもう一本
seed_snap "$SNAP_A" "20260717" # age 4 → bucket 4（新しい方は age3? wait age3 is 0718, age4 is 0717)
# age 3 and 4 both in (2,4] → keep age 3 (20260718), delete age 4 (20260717)

seed_snap "$SNAP_A" "20260621" # age 30 → bucket 32
seed_snap "$SNAP_A" "20260620" # age 31 → bucket 32 loser
seed_snap "$SNAP_A" "20250101" # age >> 256 → delete

# =============================================================================
# 実行
# =============================================================================
export OMV_TEST_FS="$FS"
export OMV_TEST_STATE="$STATE"
export OMV_BTRFS_SNAP_BIN="$BIN"
export OMV_BTRFS_SNAP_LOCK="$WORK/omv-btrfs-snap.lock"
export OMV_BTRFS_SNAP_TODAY="$TODAY_FIXED"

# 実 date 等のためシステム PATH も残す（スクリプト側で BIN を先頭に付ける）
RUN_LOG="$WORK/run1.log"
set +e
bash "$SCRIPT" >"$RUN_LOG" 2>&1
rc=$?
set -e

if [ "$rc" -eq 0 ]; then
  ok "exit code 0 on first run"
else
  ng "exit code 0 on first run" "rc=$rc; $(tail -n 20 "$RUN_LOG")"
fi

# --- OS ルート相当はスキップ、データ2本は作成 ---
assert_no_dir "OS mount has no .snapshots" "$FS/.snapshots"
assert_dir "data-a today snapshot" "$SNAP_A/daily-$TODAY_FIXED"
assert_dir "data-b today snapshot" "$BBBB/.snapshots/daily-$TODAY_FIXED"
assert_log_has "log skips OS disk mount" "skip OS btrfs" "$RUN_LOG"

# --- 刈り込み ---
# 区間 (-1,1]: age0(今日) と age1 が競合 → 今日を残す
assert_no_dir "delete age>256 (20250101)" "$SNAP_A/daily-20250101"
assert_dir "keep today in (-1,1] (20260721)" "$SNAP_A/daily-$TODAY_FIXED"
assert_no_dir "drop age1 loser in (-1,1] (20260720)" "$SNAP_A/daily-20260720"
assert_dir "keep age2 (20260719)" "$SNAP_A/daily-20260719"
assert_dir "keep newer in (2,4] (20260718)" "$SNAP_A/daily-20260718"
assert_no_dir "drop older in (2,4] (20260717)" "$SNAP_A/daily-20260717"
assert_dir "keep newer in (16,32] (20260621)" "$SNAP_A/daily-20260621"
assert_no_dir "drop older in (16,32] (20260620)" "$SNAP_A/daily-20260620"

# --- 同日再実行で増えない（exists） ---
RUN2="$WORK/run2.log"
before=$(find "$SNAP_A" -mindepth 1 -maxdepth 1 -type d | wc -l)
bash "$SCRIPT" >"$RUN2" 2>&1
after=$(find "$SNAP_A" -mindepth 1 -maxdepth 1 -type d | wc -l)
if [ "$before" -eq "$after" ]; then
  ok "second run does not add snapshots"
else
  ng "second run does not add snapshots" "before=$before after=$after"
fi
assert_log_has "second run logs exists" "exists:" "$RUN2"

# --- ロック中は静かに成功終了 ---
RUN3="$WORK/run3.log"
# 実 flock がある場合のみ厳密テスト
if command -v flock >/dev/null 2>&1 || [ -x /usr/bin/flock ]; then
  (
    exec 8>"$OMV_BTRFS_SNAP_LOCK"
    flock -n 8
    set +e
    bash "$SCRIPT" >"$RUN3" 2>&1
    rc3=$?
    set -e
    if [ "$rc3" -eq 0 ]; then
      ok "locked run exits 0"
    else
      ng "locked run exits 0" "rc=$rc3"
    fi
    assert_log_has "locked run message" "another run holds the lock" "$RUN3"
  )
else
  ok "skip flock contention test (no real flock)"
fi

# --- 非サブボリュームはスキップ ---
mkdir -p "$FS/srv/dev-disk-by-uuid-cccc"
# findmnt に一時追加はせず、単体で process 相当を確認するのは重いので
# btrfs show 失敗パスは aaaa を壊して再実行するより、追加マウントをモックに足す

cat >"$BIN/findmnt" <<EOF
#!/bin/bash
FS="$FS"
if [ "\${1:-}" = "-nt" ] && [ "\${2:-}" = "btrfs" ]; then
  printf '%s\\n' "\$FS"
  printf '%s\\n' "\$FS/srv/dev-disk-by-uuid-aaaa"
  printf '%s\\n' "\$FS/srv/dev-disk-by-uuid-bbbb"
  printf '%s\\n' "\$FS/srv/dev-disk-by-uuid-cccc"
  exit 0
fi
if [ "\${1:-}" = "-n" ] && [ "\${2:-}" = "-o" ] && [ "\${3:-}" = "SOURCE" ]; then
  shift 3
  target="/"
  if [ "\${1:-}" = "--target" ]; then
    target="\${2:-}"
  elif [ -n "\${1:-}" ]; then
    target="\$1"
  fi
  case "\$target" in
    "\$FS"|"/") echo "/dev/sda1" ;;
    "\$FS/srv/dev-disk-by-uuid-aaaa") echo "/dev/sdb1" ;;
    "\$FS/srv/dev-disk-by-uuid-bbbb") echo "/dev/sdc1" ;;
    "\$FS/srv/dev-disk-by-uuid-cccc") echo "/dev/sdd1" ;;
    *) exit 1 ;;
  esac
  exit 0
fi
exit 1
EOF
chmod +x "$BIN/findmnt"

# lsblk に sdd を追加
cat >"$BIN/lsblk" <<'EOF'
#!/bin/bash
opt=""
dev=""
while [ $# -gt 0 ]; do
  case "$1" in
    -no) opt="$2"; shift 2 ;;
    -n) shift ;;
    -o) opt="$2"; shift 2 ;;
    *) dev="$1"; shift ;;
  esac
done
case "$opt" in
  PKNAME)
    case "$dev" in
      /dev/sda1) echo "sda" ;;
      /dev/sdb1) echo "sdb" ;;
      /dev/sdc1) echo "sdc" ;;
      /dev/sdd1) echo "sdd" ;;
    esac
    ;;
  NAME)
    basename "$dev"
    ;;
esac
EOF
chmod +x "$BIN/lsblk"

RUN4="$WORK/run4.log"
bash "$SCRIPT" >"$RUN4" 2>&1
assert_log_has "skip non-subvolume mount" "skip not a subvolume mount" "$RUN4"
assert_no_dir "cccc has no snapshots" "$FS/srv/dev-disk-by-uuid-cccc/.snapshots"

# =============================================================================
# 結果
# =============================================================================
printf '\n%s\n' "---- summary ----"
printf 'passed=%s failed=%s work=%s\n' "$pass" "$fail" "$WORK"
# cleanup 前に失敗時は残したいので、失敗時は trap 解除
if [ "$fail" -ne 0 ]; then
  trap - EXIT
  printf 'artifacts kept at %s\n' "$WORK"
  printf 'run1:\n'; cat "$RUN_LOG"
  exit 1
fi
exit 0
