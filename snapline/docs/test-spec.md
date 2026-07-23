# Snapline テスト仕様書

対象バージョン: リポジトリ現行（`snapline` 0.1.0 系）  
方針: 確実な履歴バックアップ。あいまいな成功は不合格とする。

## 1. 目的

本仕様は、Snapline が次を満たすことを確認するための試験項目である。

- 明示除外以外を落とさない
- 壊れた履歴を残さない（変化検知・検証失敗はエラー）
- CLI の意味が明確で、誤用を黙って通さない
- 通常実行と `--background` で結果（包含・検証）が同じ

## 2. 試験区分

| 区分 | 内容 | 実施手段 |
| --- | --- | --- |
| U | 単体試験 | `cargo test` |
| I | 統合・E2E 試験 | `docs/run_e2e_tests.ps1`（本仕様に対応） |
| S | 静的検査 | `cargo clippy -- -D warnings` |

## 3. 環境前提

- OS: Windows 10 以降（本リポジトリの主検証環境）
- 実行ファイル: `target/release/snapline.exe`
- 作業は一時ディレクトリ上で行い、終了後に削除する

## 4. 単体試験（U）と対応

| ID | 確認内容 | 実装テスト |
| --- | --- | --- |
| U-01 | 既定除外に Git 関連名が無い | `settings::defaults_never_exclude_git_related_names` |
| U-02 | 既定では `.git` を除外しない | `settings::defaults_keep_dot_git_directory` |
| U-03 | 明示追加すれば `.git` を除外できる | `settings::can_exclude_dot_git_by_adding_to_exclude_list` |
| U-04 | ディレクトリ名除外は名前一致 | `settings::does_not_treat_files_by_path_semantics_here` |
| U-05 | ファイル名除外 | `settings::excludes_configured_file_names` |
| U-06 | 拡張子除外（`.` 有無同一視） | `settings::excludes_configured_extensions` |
| U-07 | 階層 `.snaplinenore` | `snaplinenore::applies_nested_snaplinenore_files` |
| U-08 | `.snaplinenore` で `.git` 除外可 | `snaplinenore::snaplinenore_can_exclude_dot_git` |
| U-09 | snapshot で dir 除外と `.git` 保持 | `snapshot::excludes_named_dirs_but_keeps_git` |
| U-10 | snapshot で nested snaplinenore | `snapshot::applies_nested_snaplinenore_during_snapshot` |
| U-11 | snapshot で file/ext 除外 | `snapshot::excludes_configured_file_names_and_extensions` |
| U-12 | 圧縮と復元 | `object::compresses_repetitive_content_and_restores_it` |
| U-13 | 非圧縮保持 | `object::keeps_incompressible_content_raw` |
| U-14 | 旧形式オブジェクト読取 | `object::reads_legacy_headerless_object` |
| U-15 | 相対パス検証（許可） | `restore::accepts_normal_relative_path` |
| U-16 | `..` 拒否 | `restore::rejects_parent_traversal` |
| U-17 | 絶対パス拒否 | `restore::rejects_absolute_path` |
| U-18 | 直下 `.snapline` init | `store::initializes_store_directly_under_target_tree` |
| U-19 | 外部ストア＋ポインタ | `store::initializes_external_store_with_pointer` |
| U-20 | ツリー移動後 open | `store::opens_store_after_target_tree_is_moved` |
| U-21 | 親方向 discover | `store::discovers_tree_root_from_nested_directory` |
| U-22 | 短縮 ID 一意解決 | `store::resolves_unique_short_snapshot_id` |
| U-23 | 短縮 ID 曖昧拒否 | `store::rejects_ambiguous_short_snapshot_id` |
| U-24 | CLI: log / log -1 / config / init / store / install / `--background` / 旧 `list` 拒否 | `main::tests::*` |
| U-25 | background しきい値範囲外拒否 | `background::rejects_invalid_limits` |
| U-26 | 進捗フェーズ行は常に書く | `progress::begin_and_step_always_write_lines` |
| U-27 | log は entries を件数だけ読む（旧要約補完用） | `inspect::counts_entries_without_materializing_them` |
| U-28 | write_manifest が要約も書く | `inspect::write_manifest_also_writes_summary` |
| U-29 | 旧ストアの要約欠落を log 時に補完 | `inspect::migrates_missing_summaries_on_log` |
| U-30 | log の newest 制限 | `inspect::list_log_rows_respects_newest_limit` |
| U-31 | background スモーク / 高 CPU しきい値 | `background::activate_and_pace_smoke` 他 |

合否: 全件 pass。1 件でも fail なら不合格。

## 5. 統合・E2E 試験（I）

各項目は「手順」「期待結果」を持つ。実装スクリプトが期待と異なる終了コードや内容なら不合格。

### I-01 init（直下ストア）

- 手順: 空ツリーで `snapline init <tree>`
- 期待: `<tree>/.snapline/config.json` が存在し、終了コード 0

### I-02 snapshot / log / 短縮 ID restore

- 手順: ファイルを置き `snapshot` → `log` → 短縮 ID で空先へ `restore`
- 期待: restore 先に同一内容。log 先頭 ID は 12 文字

### I-03 サブディレクトリからの実行

- 手順: `<tree>/a/b` から `--tree` 無しで `log`
- 期待: 親の `.snapline` を発見し一覧成功

### I-04 `.gitignore` を見ない

- 手順: `.gitignore` に `secret.env`、実ファイルを置き snapshot
- 期待: restore 後に `secret.env` が存在する

### I-05 子 `.snapline` は親 snapshot に含まれる

- 手順: 親 init、子に別 `.snapline`（ダミーファイル可）を置き親で snapshot/restore
- 期待: restore 先に子の `.snapline` 配下がある

### I-06 親ストア本体は除外

- 手順: snapshot 後、マニフェスト／restore 結果に親 `.snapline` の objects 等が無い
- 期待: restore 先直下に `.snapline` ストア本体が現れない（ポインタ／本体とも履歴対象外）

### I-07 exclude_file_names / exclude_extensions

- 手順: config に `Thumbs.db` と `.log` を追加して snapshot
- 期待: restore にそれらが無い。通常ファイルは残る

### I-08 外部ストア

- 手順: `--store <external>` で init → snapshot → open 相当の `log`
- 期待: ツリー側 `.snapline` はファイル（ポインタ）、本体は external 配下

### I-09 restore は非空先を拒否

- 手順: 中身のあるディレクトリへ restore
- 期待: 終了コード非 0

### I-10 曖昧な短縮 ID を拒否

- 手順: 同一 UUID 接頭辞を持つ 2 マニフェスト相当を用意できない場合は、短すぎる／存在しない ID で拒否を確認
- 期待: 存在しない短縮 ID、または曖昧 ID で非 0（実装は一意でなければエラー）

### I-11 verify

- 手順: 正常ストアで `verify`
- 期待: 終了コード 0、snapshots/objects 数が表示される

### I-12 `--background` 位置

- 手順: `snapline --background snapshot` と `snapline snapshot --background` の両方
- 期待: どちらも終了コード 0

### I-13 `--background` 誤用拒否

- 手順: `snapline --background log`
- 期待: 終了コード非 0

### I-16 `--background verify`

- 手順: 正常ストアで `snapline --background verify`
- 期待: 終了コード 0

### I-17 `--background restore`

- 手順: `snapline --background restore <id> <empty-dir>`
- 期待: 終了コード 0、内容一致

### I-18 `--background` + CPU 負荷

- 手順: バックグラウンド CPU 負荷ジョブ実行中に `--background --cpu-busy-percent 99 snapshot`
- 期待: 終了コード 0（高しきい値ならゲーム中でも完了できる）

### I-19 snapshot 要約の別保存

- 手順: snapshot 後に `.snapline/summaries/<id>.json` が存在することを確認
- 期待: 要約に `entry_count` があり、マニフェスト件数と一致

### I-20 旧ストア互換（要約欠落の自動補完）と `log -1`

- 手順: 要約ファイルを削除してから `log` → 要約が再生成される。続けて複数 snapshot 後に `log -1`
- 期待: 要約が復活し、`log -1` は最新 1 件のみ表示

### I-14 二重 init 拒否

- 手順: 同一ツリーで init を 2 回
- 期待: 2 回目は非 0

### I-15 自動 init しない

- 手順: 未 init ツリーで `snapshot`
- 期待: 非 0（勝手に `.snapline` を作らない）

## 6. 静的検査（S）

| ID | 内容 | 合否 |
| --- | --- | --- |
| S-01 | `cargo clippy -- -D warnings` が成功 | 警告をエラー扱い |

## 7. 実施結果

実施日・結果は下記「実施記録」に追記する。  
スクリプト: [run_e2e_tests.ps1](run_e2e_tests.ps1)

### 実施記録

| 日時 (UTC+9) | U | I | S | 備考 |
| --- | --- | --- | --- | --- |
| 2026-07-22 21:00 頃 | PASS（43） | PASS（I-01〜I-20） | PASS | 要約別保存 / log -1 / 旧ストア互換 |
| 2026-07-20 14:50 頃 | PASS（31） | PASS（I-01〜I-15 全件） | PASS | `cargo test` / `run_e2e_tests.ps1` / `clippy -D warnings` |

## 8. 粒度の妥当性（証明）

### 判定基準

本プロジェクトにおける「適切な粒度」は次の3条件で定義する。

1. **製品契約ごと**に、観測可能な合格条件がある（行カバレッジ最大化ではない）
2. 失敗時に **U / I / S の層**で原因を切り分けられる
3. 非決定的・環境依存の項目を、無理に E2E 化して曖昧合格にしない

### 契約への写像

| 製品契約 | 単体 (U) | E2E (I) | 粒度の根拠 |
| --- | --- | --- | --- |
| 明示除外以外を落とさない | U-01〜U-11 | I-04〜I-07 | 規則と誤解点を分離。1本の「snapshot成功」に潰していない |
| 壊れた履歴を残さない | U-12〜U-17 | I-09〜I-11 | 成功経路と拒否経路を別 ID |
| 誤用を黙って通さない | U-23〜U-25 | I-13〜I-15 | 「失敗すること」自体が合格条件 |
| 通常と `--background` の方針一致 | ペース差し替え設計 | I-12 | 入口差のみ検証し、包含E2Eの全二重化はしない |
| ストア配置・発見 | U-18〜U-21 | I-01 / I-03 / I-08 | 直下・外部・親探索を別ケース |

### 粗すぎないこと

包含方針の誤解（`.gitignore` / 子 `.snapline` / 親ストア除外 / ファイル·拡張子除外）を
I-04〜I-07 に分解している。これが1本なら粒度不足である。

### 細かすぎないこと

- 既定ディレクトリ除外21件の個別E2Eはしない（U-01で方針＋代表）
- GPUやディスク占有のOSモックはしない（U-25とI-12/13に限定）
- ヘルプ文言の全スナップショットはしない（意味のある改名・拒否のみ）

### 意図的に薄くした項目

| 項目 | 理由 |
| --- | --- |
| 取り込み中のファイル変化 | レース条件で非決定的。制限として文書化 |
| シンボリックリンク復元のE2E | 権限依存。パス検証は U-15〜17 |
| 曖昧短縮ID衝突のE2E生成 | U-23で厳密。E2Eは欠損ID（I-10） |

### 結論

試験は製品契約単位に分解され、規則は単体、往復と誤用拒否はE2E、品質ゲートは静的検査に割り当てられている。
実施結果（U31 / I15 / clippy）は全区分 PASS であり、上記基準を満たす。
