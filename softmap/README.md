# SoftMap

PCのソフト環境とフォルダ構成を、軽量なスナップショットとして記録するツール。

ファイルの中身はバックアップせず、再インストールや故障後に「何のソフトがあったか」「どんなフォルダ構成だったか」を素早く把握し、再セットアップを支援する。

## 背景

| 課題 | 方針 |
|------|------|
| 全体バックアップは容量・時間・運用の面で現実的でない | 構成情報のみを記録する |
| 故障後に困るのはファイルの場所より「何のソフトがあったか」 | OS認識ソフトの一覧を主成果物とする |
| Registry に載らないツール・ポータブルアプリもある | ドライブ全体のファイルツリーで補完記録する |

## 目的

> **PCのソフト環境を記録し、再セットアップ時に「何を入れ直すか」を判断できるようにする。**

## 基本機能

| ID | 機能 | 説明 | 記録対象 |
|----|------|------|----------|
| BF1 | OS認識ソフトの記録 | Registry（Uninstall キー）から取得 | インストーラー経由のソフト |
| BF2 | ドライブ全体の記録 | 対象ドライブのフォルダ・ファイル名を丸ごと走査・保存 | ツール系の補完 + 全体構成の地図 |
| BF3 | 記録の保存 | スナップショットファイルへエクスポート | BF1 + BF2 の統合 |
| BF4 | 記録の参照 | 用途別ビュー（サマリー・ソフト一覧・ツール一覧） | — |
| BF5 | 復元支援レポート | チェックリスト形式の再セットアップ手順を生成 | — |

### BF1 と BF2 の関係

```
ソフト全体 = BF1（OS認識） ＋ BF2（ツール系）

BF1: Chrome, 7-Zip, VS Code 等（Registry に登録済み）
BF2: 接続中の固定ドライブのフォルダ・ファイル名を全体保存（ツール系の補完を兼ねる）
```

## スキャン範囲（デフォルト）

| 項目 | 内容 |
|------|------|
| 対象ドライブ | 接続中の**固定ドライブ**を自動検出（文字の決めつけなし） |
| 対象外 | リムーバブル / CD / ネットワーク（`--drive` で明示指定は可） |
| 各ドライブ内 | ルートから均一に再帰走査 |
| 除外例 | `Windows`, `$Recycle.Bin`, `pagefile.sys`, `.git`, `node_modules` 等 |
| 記録内容 | パス・名前のみ（ファイル中身は保存しない） |

## 対象範囲

### 含む

- **BF1:** Registry に登録されたインストール済みソフト（HKLM / HKCU）
- **BF2:** 上記スキャン範囲のファイルツリー

### 含まない

- ファイルの実データ（中身）
- レジストリ・OS設定の復元
- 自動フル復元

### 記録の粒度

| モード | 記録対象 | 位置づけ |
|--------|----------|----------|
| `all_files`（**デフォルト**） | 全フォルダ + 全ファイル名 | 全体保存（メイン） |
| `folders_and_apps`（`--light`） | 全フォルダ + `.exe` / `.lnk` のみ | 軽量モード |

## プライバシー注意（公開前に必読）

スナップショット（`.smb` / `.smap`）には次が含まれます。

- ホスト名
- インストール済みソフトの一覧（製品名・発行元・パス）
- 走査したフォルダ／ファイルの**フルパス**（`C:\Users\<ユーザー名>\...` など）

**実機で取ったスナップショットを GitHub 等に上げないでください。**  
本リポジトリの `.gitignore` は `*.smb` / `*.smap` / `build/` を除外します。

## 操作方式

**コンソール（CLI）単体 exe。** GUI は設けない。

| 項目 | 方針 |
|------|------|
| 形態 | `softmap.exe` 1本（インストール不要） |
| 設定 | 任意。探索順: `-c` → exe と同じフォルダの `softmap.conf` → カレントの `softmap.conf` |
| 権限 | 一般ユーザー（管理者不要） |

### デフォルトはシンプルに

| 原則 | 具体例 |
|------|--------|
| 最短コマンドで足りる | `softmap scan` / `softmap report xxx.smb` |
| 追加機能はオプトイン | `--checklist`, `--tools`, `--map`, `--light` 等 |
| 確認を飛ばさない | `restore` の `-y` は明示時のみ |

### 使い方

```bat
softmap scan -o snapshots\2026-07-12.smb
softmap report snapshots\2026-07-12.smb

softmap report snapshots\2026-07-12.smb --software
softmap report snapshots\2026-07-12.smb --tools
softmap report snapshots\2026-07-12.smb --checklist

softmap restore snapshots\2026-07-12.smb --target D:\Restored\ --dry-run
```

## 技術スタック

| 項目 | 選定 |
|------|------|
| 言語 | C（C11） |
| 対象OS | Windows（MVP）※ Linux では BF2 のみ開発用に動作 |
| ビルド | CMake または MinGW/MSVC 直接コンパイル |
| 配布形態 | 単一 exe |

## ビルド

詳細は [BUILD.md](BUILD.md)。

```bat
cmake -B build -G "Visual Studio 17 2022" -A x64
cmake --build build --config Release
```

テスト:

```bat
powershell -ExecutionPolicy Bypass -File scripts\run_tests.ps1
```

## ドキュメント

- [ユーザーマニュアル](docs/MANUAL.txt) — 使い方
- [テストリスト](docs/TEST_LIST.txt) — 結合テスト（`scripts/run_tests.ps1`）
- [設計書](docs/DESIGN.md) — アーキテクチャ・データ形式
- [ビルド手順](BUILD.md)

## ソース構成

```
include/softmap/   … モジュール別ヘッダ
include/softmap.h  … 傘ヘッダ
src/util/  src/core/  src/scan/  src/report/  src/restore/  src/cmd/
tests/fixtures/    … 結合テスト用サンプルツリー
```

## 実装状況

| 機能 | 状態 |
|------|------|
| `scan`（BF1 + BF2、固定ドライブ自動検出） | 実装済 |
| `.smb` / `.smap` 保存・読込（depth 含む） | 実装済 |
| `report` サマリー + オプトイン詳細 | 実装済 |
| `restore --dirs-only` / `--dry-run` / `--map` | 実装済 |
| `info` | 実装済 |
| 長パス（`\\?\`） | 実装済 |
| zlib 圧縮 | 未実装（フラグ予約のみ） |
| HTML レポート / UWP / diff | 未実装 |

## ライセンスと免責

- ライセンス: [MIT License](LICENSE)
- 免責（日本語）: [DISCLAIMER.txt](DISCLAIMER.txt)

本ソフトウェアは無保証（AS IS）です。スキャン漏れ、スナップショットの取り扱い、
`restore` によるディレクトリ作成の結果などについて、作者は責任を負いません。
詳細は `LICENSE` および `DISCLAIMER.txt` を参照してください。
