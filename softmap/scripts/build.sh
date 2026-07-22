#!/bin/sh
# Build SoftMap without system gcc if needed (uses Zig cc fallback).
set -e
ROOT="$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)"
mkdir -p "$ROOT/build"

pick_cc() {
  if command -v gcc >/dev/null 2>&1; then
    echo "gcc"
    return
  fi
  if command -v cc >/dev/null 2>&1; then
    echo "cc"
    return
  fi
  if command -v clang >/dev/null 2>&1; then
    echo "clang"
    return
  fi
  ZIG="$ROOT/.tools/zig-linux-x86_64-0.13.0/zig"
  if [ -x "$ZIG" ]; then
    echo "$ZIG cc"
    return
  fi
  echo "No C compiler found. Install gcc, or place Zig at .tools/zig-linux-x86_64-0.13.0/" >&2
  exit 1
}

CC="$(pick_cc)"
# shellcheck disable=SC2086
$CC -std=c11 -Wall -Wextra -O2 -D_DEFAULT_SOURCE -I"$ROOT/include" \
  "$ROOT"/src/main.c \
  "$ROOT"/src/util/util.c \
  "$ROOT"/src/core/config.c \
  "$ROOT"/src/core/filter.c \
  "$ROOT"/src/core/tree.c \
  "$ROOT"/src/core/snapshot.c \
  "$ROOT"/src/scan/registry.c \
  "$ROOT"/src/scan/walker.c \
  "$ROOT"/src/report/report.c \
  "$ROOT"/src/restore/restore.c \
  "$ROOT"/src/cmd/cmd_scan.c \
  "$ROOT"/src/cmd/cmd_report.c \
  "$ROOT"/src/cmd/cmd_restore.c \
  "$ROOT"/src/cmd/cmd_info.c \
  -o "$ROOT/build/softmap"
echo "built: $ROOT/build/softmap"
