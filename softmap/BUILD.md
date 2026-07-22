# SoftMap — Build (Windows)

## Requirements

- C compiler: MSVC or MinGW-w64 (GCC)
- CMake 3.16+ (optional; can compile sources directly)

## CMake

```bat
mkdir build
cd build
cmake .. -G "MinGW Makefiles"
cmake --build .
```

Or with Visual Studio:

```bat
cmake -B build -G "Visual Studio 17 2022" -A x64
cmake --build build --config Release
```

Output: `build\softmap.exe` (or `build\Release\softmap.exe`)

## Direct GCC (MinGW)

```bat
gcc -std=c11 -O2 -Iinclude ^
  src\main.c src\util\util.c ^
  src\core\config.c src\core\filter.c src\core\tree.c src\core\snapshot.c ^
  src\scan\registry.c src\scan\walker.c ^
  src\report\report.c src\restore\restore.c ^
  src\cmd\cmd_scan.c src\cmd\cmd_report.c src\cmd\cmd_restore.c src\cmd\cmd_info.c ^
  -o softmap.exe -ladvapi32
```

## Quick test

```bat
powershell -ExecutionPolicy Bypass -File scripts\run_tests.ps1
```

Manual smoke:

```bat
softmap.exe scan -o snapshot.smb --software-only
softmap.exe report snapshot.smb
softmap.exe report snapshot.smb --software
```

Do **not** commit real `*.smb` / `*.smap` (they contain hostname and full paths). See `.gitignore`.

On Linux (dev only): Registry is skipped; use `--drive /path/to/dir` to exercise BF2.
