# Snapline

Snapline は、ディレクトリツリー全体を扱うローカル向けの内容アドレス型履歴ストアです。
目的は「確実な履歴バックアップ」です。推測による除外や、Git の入れ子リポジトリ向けの特別扱いは意図的に持たせません。

通常ファイル、ディレクトリ、空ディレクトリ、シンボリックリンク、更新日時、読み取り専用フラグを
記録します。同一内容は 1 回だけ保存し、効く場合だけ Zstandard 圧縮します。

## スナップショットの包含・除外（重要）

Snapline の snapshot は、**明示された除外以外を落とさない**ことを方針とします。
「たぶん不要だろう」という推測除外は行いません。確実な履歴バックアップのためです。

### 除外されるもの（明示されたもののみ）

| 除外元 | 対象 | 備考 |
| --- | --- | --- |
| 今開いているストア本体 | `store.root` とパスが一致するエントリ | 親ツリー直下の `.snapline`、または外部配置のストア本体 |
| `settings.exclude_dir_names` | その**名前のディレクトリ** | ファイル名は対象外。ツリー内のどこにあっても名前一致で除外 |
| `settings.exclude_file_names` | その**名前のファイル** | ディレクトリには適用しない。既定は空 |
| `settings.exclude_extensions` | その**拡張子のファイル** | `.log` と `log` は同じ。末尾拡張子のみ。既定は空 |
| 各階層の `.snaplineignore` | ルールに一致するパス | `.gitignore` 互換記法。親と子のルールを重ねて判定 |

これら以外の通常ファイル・ディレクトリは記録対象です。

### 含まれない／特別扱いしないもの（よくある誤解）

| 項目 | 挙動 |
| --- | --- |
| `.gitignore` | **見ない。** |
| 子階層の `.snapline` | **除外しない。中身ごと親スナップショットに入る** |
| 入れ子の Git リポジトリ | 特別扱いしない。`.git` は既定では除外せず含める |
| シンボリックリンク先のツリー | 辿らない。リンク自体は記録する |

例:

```text
C:\work\.snapline          ← 今使っているストア（除外）
C:\work\app\.snapline      ← 子ストア（除外しない。全部入る）
C:\work\app\src\main.rs    ← 入る
```

`C:\work` で `snapline snapshot` すると、子の `app\.snapline` も履歴に含まれます。
入れ子ストアを外したい場合だけ、`.snaplineignore` や `exclude_dir_names` で明示してください。

### 記録対象だが「全部の中身」ではないもの

| 項目 | 挙動 |
| --- | --- |
| シンボリックリンク | リンクパスとリンク先文字列を記録。リンク先配下は辿らない |
| 読み取り失敗 | 黙ってスキップしない。スナップショット全体を中断する |
| 非対応の特殊エントリ | ファイル／ディレクトリ／シンボリックリンク以外はエラーで中断 |
| 読み取り中に内容が変化したファイル | 中断する（壊れた履歴を残さない） |

## コマンド一覧と使い方

Snapline が提供するコマンドは `init`、`snapshot`、`log`、`restore`、`verify`、`config`、`install` です。
`init` でストアを作り、`snapshot` で記録します。

共通オプション:

| オプション | 環境変数 | 意味 |
| --- | --- | --- |
| `--tree <PATH>` | `SNAPLINE_TREE` | 対象ツリー。省略時はカレントから親方向へ `.snapline` を探す |
| `--store <PATH>` | `SNAPLINE_STORE` | ストア配置先。省略時は `<tree>/.snapline` |
| `--background` | （なし） | `snapshot` / `restore` / `verify` を低優先度・資源監視付きで実行 |

`--tree` を省略したときは、カレントディレクトリから親へ順にたどり、
最初に見つかった `.snapline` のあるフォルダを対象ツリーにします。
そのため、対象ツリー内のサブフォルダからも各コマンドを実行できます。

### `install` — PATH に登録する

`git commit` のように、実行ファイルの場所を書かずに使うには、一度だけインストールします。

```powershell
cargo build --release
.\target\release\snapline.exe install
```

`%LOCALAPPDATA%\Snapline\bin` へコピーし、ユーザー PATH に登録します。
**新しいターミナル**を開いたあと:

```powershell
snapline --help
```

以降は `snapline` を直接呼べます。

### `init` — 履歴ストアを作成する

```powershell
# 対象ツリー直下に .snapline を作る
snapline init C:\work

# カレントが対象ツリーのとき
snapline init

# 直下以外へ .snapline を置く（C:\stores\work\.snapline が本体）
# ツリー側にはポインタファイル C:\work\.snapline が残る
snapline --tree C:\work --store C:\stores\work init
```

### `snapshot` — 現在のツリーを記録する

```powershell
snapline --tree C:\work snapshot
snapline --tree C:\work snapshot -m "Before migration"

# 対象ツリー内のサブフォルダでも --tree を省略できる
Set-Location C:\work\projectA\src
snapline snapshot -m "daily"
```

### `log` — スナップショット一覧

```powershell
snapline --tree C:\work log

# 最新だけ表示
snapline --tree C:\work log -1
snapline --tree C:\work log -n 1
```

一覧の先頭には、末尾 UUID 部分から作った 12 文字の短縮 ID が表示されます。
進捗は stderr、一覧そのものは stdout に出ます。
表示に使う要約は `.snapline/summaries/` に別保存され、巨大なマニフェスト本体は読みません。
旧ストアに要約が無い場合は、`log`（または次の `snapshot`）実行時にマニフェストから自動生成します。

### `restore` — 空の場所へ復元する

```powershell
# log に表示された短縮 ID をそのまま指定できる
snapline restore a1b2c3d4e5f6 D:\restored-work
```

完全 ID、完全 ID の先頭、UUID 部分の先頭のいずれでも指定できます。
短縮 ID は 4 文字以上必要で、複数の履歴に一致する場合は安全のため拒否します。
復元先は新規または空ディレクトリのみです。既存ツリーは上書きしません。

### `verify` — 履歴の整合性を確認する

```powershell
snapline --tree C:\work verify
```

### `config` — 現在の設定を表示する

```powershell
snapline --tree C:\work config
```

設定の変更は `.snapline/config.json`（外部配置時はストア本体側）を編集します。


### `--background` — 低優先度オプション

`snapshot` / `restore` / `verify` に付けて使う。
ゲームやブラウジング中に、表の処理を邪魔しにくくする。
ペース（待機）だけが変わり、包含・除外・検証ルールは通常と同じ。

```powershell
snapline --background snapshot -m "while gaming"
snapline --background restore a1b2c3d4e5f6 D:\restored-work
snapline --background verify

# しきい値を明示する場合
snapline --background --cpu-busy-percent 60 snapshot
```

`init` / `log` / `config` / `install` に `--background` を付けるとエラーになる（無視して続行しない）。

`--poll-ms` や `--cpu-busy-percent` などは **`--background` と一緒に付けたときだけ** 有効です。単独では無視されます。

しきい値（省略時は既定値）:

| オプション | 既定 | 意味 |
| --- | --- | --- |
| `--cpu-busy-percent` | `70` | 全体 CPU 使用率がこれを超えたら待機 |
| `--memory-load-percent` | `90` | 物理メモリ使用率がこれを超えたら待機 |
| `--poll-ms` | `200` | 待機中の再確認間隔（ミリ秒） |

動作:

- プロセスを Windows のバックグラウンド優先度へ下げる（失敗したらエラーで止まる）
- CPU / メモリを定期的に監視し、空くまで待ってから I/O を進める
- 監視や優先度変更に失敗した場合、黙って通常優先度で続行することはしない

## `.snaplineignore`（gitignore 互換の除外）

`.gitignore` には追従しません。代わりに Snapline 専用の `.snaplineignore` を使います。
詳細な包含方針は「スナップショットの包含・除外（重要）」を参照してください。

- 記法は `.gitignore` と同じ
- ツリー内の**すべての階層**にある `.snaplineignore` が有効
- 親のルールと子のルールを重ねて判定する（子の否定パターン `!` も有効）
- `exclude_dir_names` のディレクトリ名除外も並行して効く
- `.git` を落としたい場合は `exclude_dir_names` や `.snaplineignore` で明示する

例:

```text
# C:\work\.snaplineignore
*.log
*.tmp

# C:\work\app\.snaplineignore
cache/
build/
```

## ストア配置

### 既定（ツリー直下）

```text
C:\work\
  .snapline\          ← ストア本体
    config.json
    objects\
    snapshots\
    summaries\
    tmp\
  projectA\
  projectB\
```

### 外部配置（`--store`）

```text
C:\work\
  .snapline           ← ポインタファイル（本体場所を指す）
  projectA\

C:\stores\work\
  .snapline\          ← ストア本体
    config.json
    objects\
    snapshots\
    summaries\
    tmp\
```

`--store C:\stores\work` と書くと `C:\stores\work\.snapline` が作られます。  
すでに `.snapline` で終わるパスを渡せば、そのパス自体がストア本体になります。

除外されるのは**今開いているストア本体だけ**です。
子階層にある別の `.snapline` は自動除外されません（包含・除外の節を参照）。

## ユーザー設定（config.json）

| 項目 | 既定 | 意味 |
| --- | --- | --- |
| `settings.exclude_dir_names` | 再生成可能な依存・キャッシュ名 | その名前のディレクトリをツリー全体で除外 |
| `settings.exclude_file_names` | `[]` | その名前のファイルをツリー全体で除外 |
| `settings.exclude_extensions` | `[]` | その拡張子のファイルをツリー全体で除外（`.log` / `log` は同じ） |

設定の保存場所は `.snapline/config.json`（外部配置時はストア本体側）です。
変更は次の `snapshot` から反映されます。専用の設定変更コマンドはまだありません。

`.gitignore` を除外条件に使わない理由:  
「共有しないもの」と「履歴に残さないもの」は別だからです。`.env` などを誤って落とす危険を避けます。
確実な履歴バックアップのため、除外は明示指定に限定します。

## モジュール関係

ソース構成と依存の向きは [docs/modules.md](docs/modules.md) を参照してください。

## テスト

試験項目と合否基準は [docs/test-spec.md](docs/test-spec.md) を参照してください。
E2E は次で実行できます。

```powershell
cargo build --release
cargo test
cargo clippy -- -D warnings
.\docs\run_e2e_tests.ps1
```

## ビルド

```powershell
cargo build --release
```

実行ファイル: `target\release\snapline.exe`

## 現時点の制限

- 既存ツリーの上書きや削除を伴う復元はしない
- ACL、代替データストリームなど、プラットフォーム固有のメタデータは記録しない
- Windows でのシンボリックリンク作成には、Developer Mode や昇格権限が必要な場合がある
- ファイル名とシンボリックリンク先は Unicode で表現できる必要がある
- 読み取り中にファイルが変化した場合、スナップショット作成は中断する
- 設定変更用の専用サブコマンドはまだなく、`config.json` の直接編集が必要
