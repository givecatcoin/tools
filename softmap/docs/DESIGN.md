# SoftMap 設計書

## 1. システム概要

```
┌──────────────────────────────────────────────────────────┐
│                      SoftMap CLI                         │
├────────────┬─────────────┬──────────────┬────────────────┤
│   scan     │   export    │    report    │    restore     │
│  (収集)    │   (保存)    │   (参照)     │   (再現支援)   │
├────────────┴─────────────┴──────────────┴────────────────┤
│                    コアライブラリ                          │
│  registry │ walker │ filter │ tree │ snapshot │ report  │
├──────────────────────────────────────────────────────────┤
│                   プラットフォーム層                        │
│            win32_registry │ win32_walker │ win32_path     │
└──────────────────────────────────────────────────────────┘
```

### 処理フロー

```
[scan]
  ├─ Registry 走査 ──→ BF1: OS認識ソフト一覧
  └─ ファイル走査  ──→ BF2: ドライブ全体のファイルツリー
         │
         ▼
    [snapshot 統合]
         │
         ▼
    .smb ファイル保存
         │
    [report / restore]
         ├─ ソフト一覧（BF1）+ ツール一覧（BF2）
         ├─ チェックリスト生成
         └─ フォルダ再現（dirs-only）
```

### BF1 / BF2 の役割分担

| | BF1（Registry） | BF2（ファイル走査） |
|--|----------------|-------------------|
| 主目的 | インストール済みソフトの記録 | **ドライブ全体のファイルツリー保存** |
| 副次効果 | — | ツール・ポータブルアプリの補完把握 |
| 例 | Chrome, 7-Zip, Office | 固定ドライブ上の全フォルダ・ファイル名 |
| 精度 | 製品名・バージョンが正確 | パス・ファイル名そのもの |
| 権限 | 一般ユーザーで可 | 一般ユーザーで可 |

BF2 は対象ドライブを**均一なルールで全体走査**する。ドライブ文字による特別扱いは行わない。

---

## 2. モジュール構成

```
SoftMap/
├── include/
│   ├── softmap.h              # 傘ヘッダ（全モジュールまとめ）
│   └── softmap/
│       ├── types.h             # 共通型・定数
│       ├── util.h
│       ├── config.h
│       ├── filter.h
│       ├── snapshot.h          # ツリー操作 + .smb/.smap I/O
│       ├── registry.h          # BF1
│       ├── walker.h            # BF2
│       ├── report.h
│       ├── restore.h
│       └── cmd.h               # CLI コマンド
├── src/
│   ├── main.c
│   ├── util/
│   │   └── util.c
│   ├── core/
│   │   ├── config.c
│   │   ├── filter.c
│   │   ├── tree.c
│   │   └── snapshot.c
│   ├── scan/
│   │   ├── registry.c
│   │   └── walker.c
│   ├── report/
│   │   └── report.c
│   ├── restore/
│   │   └── restore.c
│   └── cmd/
│       ├── cmd_scan.c
│       ├── cmd_report.c
│       ├── cmd_restore.c
│       └── cmd_info.c
├── docs/
│   └── DESIGN.md
├── tests/
│   └── fixtures/
├── CMakeLists.txt
└── README.md
```

各 `.c` は対応する `include/softmap/*.h` を直接 include する。`softmap.h` は外部・簡易用の傘ヘッダ。

---

## 3. データ構造

### 3.1 ソフトエントリ

```c
typedef struct sm_software {
    char     *name;           /* DisplayName: "Google Chrome" */
    char     *version;        /* DisplayVersion: "128.0.xxx" */
    char     *publisher;      /* Publisher */
    char     *location;       /* InstallLocation */
    char     *uninstall_key;  /* Registry キー名（識別用） */
    int       scope;          /* 0=HKLM, 1=HKCU */
    struct sm_software *next;
} sm_software_t;
```

### 3.2 ツリーエントリ

```c
typedef enum {
    SM_DIR,
    SM_FILE,
    SM_LINK          /* .lnk（v2） */
} sm_node_type_t;

typedef struct sm_node {
    sm_node_type_t  type;
    char           *path;     /* 相対パス: "work\\projects\\main.c" */
    char           *name;     /* 末尾名: "main.c" */
    uint64_t        size;     /* ファイルサイズ（オプション） */
    int64_t         mtime;    /* 更新日時（オプション） */
    struct sm_node *next;     /* フラットリスト */
} sm_node_t;
```

### 3.3 スナップショット

```c
typedef struct sm_snapshot {
    /* メタ情報 */
    char       *hostname;
    int64_t     scan_time;    /* Unix epoch */
    char       *config_hash;  /* 使用した設定の識別子 */

    /* BF1: ソフト一覧 */
    sm_software_t *software;
    uint32_t       software_count;

    /* BF2: ツリー */
    sm_node_t     *nodes;
    uint64_t       node_count;

    /* 統計（レポート用） */
    uint32_t       dir_count;
    uint32_t       file_count;
} sm_snapshot_t;
```

**設計判断:** ツリーはフラットリストで保持する。親子関係はパス文字列から導出する。ポインタツリーは v2 以降で検討。

---

## 4. スナップショットファイル形式（.smb）

テキスト（.smap）とバイナリ（.smb）の2形式を用意する。本番は .smb を使用。

### 4.1 バイナリヘッダ（64バイト固定）

```
Offset  Size  Field
──────  ────  ─────
0       8     magic        "SMAP001\0"
8       1     version_major uint8 (1)
9       1     version_minor uint8 (0)
10      1     depth_mode   uint8 (0=all_files, 1=folders_and_apps)
11      1     reserved
12      4     flags        uint32
16      8     scan_time    int64 (Unix epoch)
24      4     sw_count     uint32  ソフト件数
28      4     reserved
32      8     node_count   uint64  ツリーノード件数
40      4     dir_count    uint32  ディレクトリ数
44      4     file_count   uint32  ファイル数
48      16    hostname     char[16]
```

flags ビット定義:
- bit 0: size 情報あり
- bit 1: mtime 情報あり
- bit 2: zlib 圧縮（未実装・予約）

### 4.2 ソフトウェアセクション

```
[Software Record — 可変長]
  scope:       uint8   (0=HKLM, 1=HKCU)
  name_len:    uint16
  name:        UTF-8
  version_len: uint16
  version:     UTF-8
  location_len: uint16
  location:    UTF-8
  publisher_len: uint16
  publisher:   UTF-8
```

### 4.3 ツリーセクション

```
[Tree Node Record — 可変長]
  type:      uint8   (0=dir, 1=file, 2=link)
  path_len:  uint16
  path:      UTF-8
  [opt] size:  uint64  (flags bit 0)
  [opt] mtime: int64   (flags bit 1)
```

### 4.4 テキスト形式（.smap）— デバッグ・互換用

```text
# SoftMap v1
# scan: 2026-07-11T21:00:00+09:00
# host: MYPC

[software]
HKLM	Google Chrome	128.0.6613.120	C:\Program Files\Google\Chrome	Google LLC
HKCU	Visual Studio Code	1.92.0	C:\Users\Alice\AppData\Local\Programs\Microsoft VS Code	Microsoft Corporation

[tree]
D	C:\Tools
D	C:\Tools\bin
F	C:\Tools\bin\helper.exe
D	C:\Data
F	C:\Data\readme.txt
```

---

## 5. スキャン設計

### 5.1 Registry スキャン（BF1）

対象キー:

```
HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*
HKLM\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*
HKCU\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*
```

各サブキーから取得する値:

| 値名 | 必須 | 用途 |
|------|------|------|
| DisplayName | ○ | 製品名。空ならスキップ |
| DisplayVersion | △ | バージョン |
| InstallLocation | △ | インストール先 |
| Publisher | △ | 発行元 |
| SystemComponent | - | 1 ならスキップ（Windows 構成要素） |
| ParentKeyName | - | 存在すればスキップ（子エントリ） |

フィルタルール:
- `DisplayName` が空のエントリは除外
- `SystemComponent = 1` は除外
- `ParentKeyName` が存在するエントリは除外
- Windows Update 用の KB エントリ（`DisplayName` が "KB..." で始まる）は除外

### 5.2 ファイル走査（BF2: ドライブ全体保存）

対象ドライブのフォルダ・ファイル名を**丸ごと記録**する。BF2 のメインは全体保存であり、ツール系の補完把握はその結果から行う。

#### 設計方針

```
各ドライブをルートから均一に再帰走査
  └─ 除外リストに該当するパスのみスキップ
  └─ それ以外はすべて記録
```

| 項目 | 方針 |
|------|------|
| 走査単位 | ドライブ（接続中のローカルドライブ） |
| 特別ルール | **なし** — 全ドライブ・全深さで同一ルール |
| システムドライブ | 文字を決めつけない（自動検出） |
| 記録内容 | 全フォルダ名 + 全ファイル名（中身は含まない） |
| サイズ抑制 | 除外リスト + 圧縮（ファイル中身は保存しない） |

#### 走査アルゴリズム

```
1. 設定の drives リストを展開（デフォルト: 接続中の固定ドライブを自動検出）
2. 各ドライブのルートから再帰走査を開始
3. パスが exclude に該当すればスキップ（サブツリーも除外）
4. 該当しなければエントリを記録して子を走査
5. depth 設定に従いファイルを記録（all_files / folders_and_apps）
```

#### デフォルト走査ドライブ

```
接続中の固定ドライブ（DRIVE_FIXED / RAMDISK）を自動検出
※ リムーバブル・CD-ROM・ネットワークは除外（--drive で明示指定は可）
※ drives = auto でも自動検出
```

#### 除外リスト（全ドライブ・全深さに均一適用）

パス前方一致またはフォルダ名一致で判定する。

```
Windows              ; OS 本体（巨大・ソフト把握に不要）
$Recycle.Bin
System Volume Information
PerfLogs
Recovery
Config.Msi
pagefile.sys
hiberfil.sys
node_modules         ; 任意: 開発ノイズ削減
.git
```

`include` リスト・`scan_mode = narrow` は任意機能とし、通常は使用しない。

#### 記録粒度

| モード | 記録対象 | 位置づけ |
|--------|----------|----------|
| `all_files`（**デフォルト**） | 全ディレクトリ + 全ファイル名 | 全体保存（メイン） |
| `folders_and_apps` | 全ディレクトリ + `.exe` + `.lnk` | 軽量モード（オプション） |

#### サイズ見積もり

| 条件 | 目安 |
|------|------|
| C: + D: / all_files | 数百 MB〜数 GB（圧縮後。ファイル数に依存） |
| C: + D: / folders_and_apps | 数十〜数百 MB（圧縮後） |

ファイル中身を保存しないため、実データバックアップよりはるかに軽量。`Windows\` 除外でサイズを大幅に抑制する。

#### 走査時のエラー処理

| エラー | 処理 |
|--------|------|
| ERROR_ACCESS_DENIED | スキップしてログ記録 |
| パス長 > 260 | `\\?\` プレフィックスで Win32 API を呼ぶ（実装済） |
| シンボリックリンク | 訪問済みセットで循環防止。記録はするが再帰しない |

---

## 6. レポート設計（BF4 / BF5）

### 6.1 レポート種別

| コマンド | 出力 |
|----------|------|
| `report` | サマリー（BF1 + BF2 の概要） |
| `report --software` | BF1: OS認識ソフト一覧 |
| `report --tools` | BF2: 全体記録から exe/lnk を抽出したツール一覧 |
| `report --tree [--depth N]` | ツリー表示 |
| `report --checklist` | 再セットアップ用チェックリスト |

### 6.2 サマリー出力例

```text
═══════════════════════════════════════
 SoftMap レポート
 スキャン: 2026-07-11 21:00  ホスト: MYPC
═══════════════════════════════════════

[ソフトウェア] 42 件（BF1: Registry）
  HKLM: 28 件 / HKCU: 14 件

[ツール系] 18 件（BF2: exe/lnk から抽出）
  C:\Tools\ffmpeg.exe
  D:\Utils\portable-app.exe
  ...

[ドライブ統計]
  C:\  dirs: 85,000 / files: 420,000
  D:\  dirs: 12,000 / files: 95,000

[記録範囲]
  ドライブ: C:\, D:\  /  モード: all_files（全体保存）

[統計]
  合計: ディレクトリ 97,000 / ファイル 515,000
  スナップショットサイズ: 850 MB（圧縮後）
```

### 6.3 チェックリスト出力例

```text
═══ 再セットアップ チェックリスト ═══

■ ソフトの再インストール（BF1: Registry）
  [ ] Google Chrome  (128.0)
  [ ] 7-Zip  (24.08)
  [ ] Visual Studio Code  (1.92)
  ...

■ ツール・ポータブルアプリの復元（BF2: 手動コピー）
  [ ] C:\Tools\ffmpeg.exe       → ポータブル版を再配置
  [ ] C:\Tools\bin\helper.exe   → 自作ツールを再配置
  [ ] D:\Utils\portable-app.exe → 元の場所にコピー
  ...

■ フォルダの再作成
  [ ] C:\Tools
  [ ] C:\Data
  [ ] D:\Utils
  ...
```

### 6.4 ツリー表示

- デフォルト深さ: 3階層
- それ以深は件数のみ: `(15 items)`
- インデント: 2スペース

---

## 7. 復元設計

### 7.1 方針

- ファイルの中身は復元しない
- デフォルトは `--dirs-only`（空フォルダのみ作成）
- 必ず `--dry-run` オプションを提供
- 本番実行時は確認プロンプト

### 7.2 パスマッピング

```bash
softmap restore snapshot.smb --target D:\Restored\ --dry-run
softmap restore snapshot.smb --target D:\Restored\ \
  --map "C:\Tools=D:\Restored\Tools"
```

- `--map` の置換先が**絶対パス**のときは、それを最終パスとして使う（`--target` に再度載せない）
- 置換されなかったパス、または相対の置換先は `--target` 配下に配置する
- マッピングは最長一致で適用する

### 7.3 処理フロー

```
1. スナップショット読み込み
2. パスマッピング適用
3. ディレクトリエントリをソート（浅い順）
4. dry-run: 作成予定一覧を出力
5. 実行: CreateDirectoryW（既存はスキップ）
6. 結果レポート出力
```

---

## 8. 設定ファイル

`softmap.conf`（INI形式）:

```ini
[software]
; 現時点では registry のみ
source = registry

[tree]
; 走査対象ドライブ（省略時は固定ドライブを自動検出）
; drives = auto
; drives = E:\

; 除外は組み込みデフォルトに追加（置き換えではない）
exclude = Windows
exclude = $Recycle.Bin
exclude = System Volume Information
exclude = PerfLogs
exclude = Recovery
exclude = Config.Msi
exclude = node_modules
exclude = .git

; 記録粒度: all_files（全体保存）| folders_and_apps（軽量）
depth = all_files

; report 詳細は CLI のオプトインフラグで指定（設定で強制しない）

[output]
; text | binary
format = binary
compress = true
```

環境変数（`%APPDATA%` 等）は読み込み時に展開する。
設定ファイル自体も任意。無い場合は組み込みデフォルト（固定ドライブ自動検出 + 標準除外）で動作する。
設定の探索順: `-c` 指定 → exe と同じフォルダの `softmap.conf` → カレントの `softmap.conf`。

---

## 9. 操作方式（コンソール）

操作は **コンソール CLI のみ**。GUI・対話メニュー・ウィザードは設けない。

### 9.1 UX 原則: 機能は用意するがデフォルトで強制しない

| 原則 | 内容 |
|------|------|
| 最短経路 | `scan` / `report` の2コマンドで日常運用が完結する |
| オプトイン | 追加ビュー・軽量モード・パスマッピング等はフラグ指定時のみ有効 |
| 設定は任意 | `softmap.conf` が無くても組み込みデフォルトで動作する |
| 安全側 | `restore` は確認プロンプトあり。`-y` や破壊的挙動は明示時のみ |
| 出力は控えめ | `report` のデフォルトは短いサマリー。詳細は `--software` 等で要求 |

```
デフォルト体験:
  softmap scan -o out.smb     → BF1+BF2 を記録して終わる
  softmap report out.smb      → サマリーだけ表示

オプトイン例（使いたい人だけ）:
  --software / --tools / --checklist / --tree
  --light / --software-only / --map / -y
```

### 9.2 CLI 仕様

```
softmap <command> [options]

Commands:
  scan     スキャンしてスナップショットを生成
  report   スナップショットからレポートを生成
  restore  フォルダ構造を再現
  info     スナップショットのメタ情報を表示

Global Options:
  -h, --help       ヘルプ
  -v, --verbose    詳細ログ（オプトイン）
  -q, --quiet      エラーのみ（オプトイン）

scan:
  -o, --output <file>    出力ファイル（省略時は日付名を提案 or カレントに生成）
  -c, --config <file>    設定ファイル（任意。無くてもデフォルト動作）
  --software-only        Registry のみ（オプトイン）
  --light                軽量走査 folders_and_apps（オプトイン）
  --drive <letter>       走査ドライブ上書き（オプトイン）

report:
  <snapshot>             スナップショットファイル
  （引数なし）            短いサマリーのみ ← デフォルト
  --software             BF1 ソフト一覧（オプトイン）
  --tools                BF2 ツール系一覧（オプトイン）
  --tree                 ツリー表示（オプトイン）
  --depth <n>            ツリー深さ（--tree 時のみ意味あり）
  --checklist            チェックリスト（オプトイン）
  -O, --output <file>    ファイル出力（省略時は stdout）

restore:
  <snapshot>             スナップショットファイル
  --target <dir>         復元先ルート（必須）
  --dirs-only            ディレクトリのみ（デフォルト動作）
  --dry-run              実行せず一覧表示（推奨・強制はしない）
  --map <old=new>        パス置換（オプトイン）
  -y, --yes              確認プロンプトをスキップ（オプトイン。デフォルトは確認あり）
```

### 9.3 デフォルト vs オプトイン一覧

| 機能 | デフォルト | 有効化方法 |
|------|-----------|------------|
| BF1 + BF2 スキャン | ○ | `scan` |
| サマリー表示 | ○ | `report` |
| ソフト詳細一覧 | × | `--software` |
| ツール一覧 | × | `--tools` |
| チェックリスト | × | `--checklist` |
| ツリー全文表示 | × | `--tree` |
| 軽量スキャン | × | `--light` |
| BF1 のみ | × | `--software-only` |
| パスマッピング | × | `--map` |
| 確認スキップ | × | `-y` |
| 詳細ログ | × | `-v` |

---

## 10. エラーハンドリング

| 状況 | 対応 |
|------|------|
| Registry キー読み取り失敗 | スキップして verbose ログ |
| ディレクトリアクセス拒否 | スキップ。レポート末尾に一覧 |
| 設定ファイル不在 | デフォルト設定で実行 |
| スナップショット破損 | エラー終了（exit code 2） |
| 復元先が存在しない | 親から順に作成 |
| ディスク容量不足 | エラー終了。作成済みはそのまま |

終了コード:
- 0: 成功
- 1: 一般エラー
- 2: ファイル形式エラー
- 3: 部分的成功（スキップあり）

---

## 11. 文字コード

| 層 | エンコーディング |
|----|-----------------|
| Windows API | Wide char（UTF-16） |
| 内部処理 | UTF-8 |
| ファイル出力 | UTF-8（BOM なし） |
| コンソール出力 | UTF-8（Windows 10 1903+ の UTF-8 モード） |

---

## 12. テスト方針

| レベル | 内容 |
|--------|------|
| 単体 | filter, tree, config のパース・ロジック |
| 結合 | fixtures ディレクトリで scan → export → report |
| 手動 | 実際の Windows 環境で Registry + 走査 |

テスト用 fixtures 例:

```
tests/fixtures/sample_pc/
├── Tools/
│   └── ffmpeg.exe      （中身はダミー）
└── work/
    └── docs/
        └── note.txt
```

---

## 13. 実装状況（完成時点）

| 優先度 | 内容 | 状態 |
|--------|------|------|
| P0 | Registry スキャン、`.smap`、基本 CLI | 実装済 |
| P1 | walker + filter、`.smb`、設定ファイル、report サマリー | 実装済（zlib は未） |
| P2 | 固定ドライブ自動検出・均一走査、`--tools` / `--checklist` / `--tree` | 実装済 |
| P3 | `restore --dirs-only` / `--dry-run` / `--map` | 実装済 |
| P4 | HTML レポート、.lnk 詳細解析、UWP | 未実装（将来） |

スナップショット差分（diff）は未実装。

---

## 14. 制約と既知の限界

| 項目 | 限界 |
|------|------|
| ポータブルアプリ | BF2 のファイル走査で検出（Registry には無い） |
| UWP / Store アプリ | Uninstall 列挙にはほぼ載らない |
| 製品名の正確さ | BF1（Registry）は高精度。BF2 は exe 名・フォルダ名 |
| ファイル中身 | 復元しない |
| 完全自動復元 | 不可。チェックリストによる手動支援 |
| リムーバブル / ネットワーク | デフォルト走査対象外（`--drive` で明示可） |
| スナップショットの機微性 | ホスト名・フルパス・ソフト一覧を含む。**公開リポジトリにコミットしない** |

---

## 15. 将来拡張（参考）

- zlib 圧縮の実装
- 定期実行（タスクスケジューラ連携）
- 複数スナップショットの履歴管理 / diff
- JSON / CSV エクスポート
- Linux / macOS 対応（ディレクトリ走査部分は移植可能）
- GUI（TUI: ncurses）
- HTML レポート / UWP 対応
