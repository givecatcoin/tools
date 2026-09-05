# モジュール関係

Snapline の処理は、どのコマンドでも同じ流れを通る。
変わるのは「どのペースで進めるか」と「どの操作か」だけである。

## 処理の流れ

```mermaid
flowchart TB
  cli["1. main<br/>CLI を解釈する"]
  pacePick["2. pace を決める<br/>通常: IdlePace<br/>--background: BackgroundPace"]
  op["3. 操作を選ぶ<br/>snap / care / restore / browse / inspect"]
  object["4. object<br/>内容の取り込み・検証・圧縮"]
  store["5. store<br/>.snapline へ読み書き"]
  data["6. model / settings / select<br/>形・除外・パス絞り込み"]

  cli --> pacePick --> op --> object --> store --> data

  snaplineignore["snaplineignore<br/>パス除外"]
  op -.->|snap のみ| snaplineignore
  snaplineignore --> data

  background["background<br/>低優先度と資源監視"]
  pacePick -.->|--background 時| background
  background --> pacePick
```

上から下へ読む。

1. `main` がコマンドを受け取る
2. ペースを決める（通常は待機なし、`--background` なら監視付き）
3. 同じ操作モジュールを呼ぶ（`snapshot` / `care` / `restore` / `browse` / `inspect`）
4. ファイル実体は `object` が扱う
5. 置き場所とマニフェストは `store` が扱う
6. データの形・除外・パス絞り込みは `model` / `settings` / `select`（記録時は `snaplineignore` も）

`install` だけはこの流れの外で、実行ファイルの PATH 登録だけを行う。

## 通常と --background の違い

違いは **2. ペース** だけである。操作・包含除外・検証ルールは同じ。

| | 通常 | `--background` |
| --- | --- | --- |
| ペース | `IdlePace`（何もしない） | `BackgroundPace`（CPU/メモリ監視・低優先度） |
| 呼び方 | `snapline snap` など | `snapline --background snap` など |
| 本体 | 同じ `_with_pace` 経路 | 同じ `_with_pace` 経路 |

## モジュール一覧

| モジュール | 流れ上の位置 | 役割 |
| --- | --- | --- |
| `main` | 1 | CLI 解釈と振り分け |
| `progress` | 1→3 | 進捗は stderr、結果は stdout。区切りに空行 |
| `pace` | 2 | I/O 待機の共通口 |
| `background` | 2（任意） | `BackgroundPace` の実装 |
| `snapshot` | 3 | 記録（CLI 名は `snap`） |
| `care` | 3 | 整合確認＋一括圧縮 |
| `restore` | 3 | 復元（`--path` / `--dry-run`、上書きなし） |
| `browse` | 3 | `tree` / `find` |
| `select` | 6 | パスフィルタと想定容量集計 |
| `inspect` | 3 | 一覧（`log`）・検証 |
| `object` | 4 | ハッシュ化・圧縮・検証付き I/O |
| `store` | 5 | `.snapline` 構造・ロック（取得時に `tmp/` 残骸掃除）・マニフェスト |
| `model` | 6 | オンディスク JSON の型 |
| `settings` | 6 | 除外設定と既定値 |
| `snaplineignore` | 3→6 | 階層 `.snaplineignore` |
| `install` | （別） | PATH 登録 |

## 依存の向き

利用する側から利用される側へ向かう。循環はない。

```text
main
  → pace / background
  → snapshot / care / restore / browse / inspect
      → select
      → object → store → model → settings
      → pace
      → snaplineignore → settings   （snap のみ）
  → store                         （init / config）
  → install                       （install）
```

## コマンドごとの経路

すべて「ペース → 操作 → object/store」の形に揃う（閲覧系は object を触らない）。

| コマンド | 経路 |
| --- | --- |
| `init` / `config` | `main` → `store`（`init --config-only` は config.json のみ） |
| `log` | `main` → `inspect::list_log_rows`（欠落要約は自動補完）→ `store` |
| `tree` / `find` | `main` → `browse` → `select` / `store` |
| `snap` | `main` → IdlePace → `snapshot`（既定: 簡易整合・raw / `--rehash` / `--compress` を独立指定） |
| `care` | `main` → IdlePace → `care`（verify → compact） |
| `restore` | `main` → IdlePace → `restore` → `select` / `object` / `store` |
| `verify` | `main` → IdlePace → `inspect::verify`（壊れたマニフェストはスキップ報告、最終は失敗）→ `object` / `store` |
| `--background snap` など | `main` → BackgroundPace → 同じ操作 → `object` / `store` |
| `install` | `main` → `install` |

## 変更時の目安

| 変えたいこと | 見る場所 |
| --- | --- |
| CLI や振り分け | `main` |
| 待機・低優先度 | `pace` / `background` |
| 記録・手入れ・復元・検証・閲覧 | `snapshot` / `care` / `restore` / `inspect` / `browse` |
| パス絞り込み・想定容量 | `select` |
| 圧縮・ハッシュ形式 | `object`（オンディスク互換を壊さない） |
| ストア配置 | `store` |
| 除外ルール | `settings` / `snaplineignore` / README の包含節 |
