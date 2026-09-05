# Snapline テスト仕様

対象バージョン: ローカル専用 `snapline` 0.3.5 系
方針: 変更のたびに回帰しやすい項目を固定し、手動確認の抜けを減らす。

## 1. 目的

次を毎回確認し、Snapline の安全な履歴バックアップ方針が崩れていないことを担保する。

- 記録・復元・検証の基本経路
- 部分閲覧（tree / find）と部分 restore（上書きなし）
- CLI の拒否条件とヘルプの一貫性
- 必要なら `--background` の動作

## 2. 実行区分

| 区分 | 内容 | 主な実行手段 |
| --- | --- | --- |
| U | ユニット | `cargo test` |
| I | 結合 E2E | `docs/run_e2e_tests.ps1` |
| S | 静的検査 | `cargo clippy -- -D warnings` |

## 3. 前提

- OS: Windows 10 以降
- 実行バイナリ: `target/release/snapline.exe`

## 4. ユニット（U）重点（0.3.x）

| ID | 内容 | 代表テスト |
| --- | --- | --- |
| U-11a | 既定 snap の reuse（増分バイト 0） | `snapshot::reuses_unchanged_files_with_simple_consistency_check` |
| U-11b | 欠落オブジェクト時は取り込みへ | `snapshot::falls_back_to_ingest_when_object_is_missing` |
| U-11c | `--rehash` は reuse せず raw | `snapshot::rehash_reads_all_without_reuse` |
| U-11d | `--rehash --compress` で zstd | `snapshot::rehash_with_compress_stores_zstd` |
| U-11e | `--compress` 単独でも reuse | `snapshot::compress_alone_still_reuses_unchanged` |
| U-11f | `--rehash` は size/mtime 同一の改変を検出 | `snapshot::rehash_detects_same_size_content_change` |
| U-12a | raw 保存 | `object::stores_raw_without_compression` |
| U-12b | compact | `object::compact_converts_raw_repetitive_object_to_zstd` |
| U-12c | care は verify 後に圧縮 | `care::care_verifies_and_compacts_raw_objects` |
| U-12d | 破損オブジェクトで verify 失敗 | `inspect::verify_fails_when_object_payload_is_corrupted` |
| U-12e | 破損オブジェクトで care 失敗 | `care::care_fails_when_object_is_corrupted` |
| U-12f | 壊れたマニフェストはスキップ継続（読めた側は残す） | `inspect::verify_skips_*` / `inspect::list_keeps_*` |
| U-12g | write.lock 時に tmp 残骸削除 | `store::lock_cleans_stale_tmp_files` |
| U-12h | write.lock 競合時は英語のみの明確なエラー | `store::lock_reports_clear_error_when_already_held` |
| U-12i | care も壊れたマニフェストを skipped 報告 | `care::care_reports_skipped_broken_manifest_without_dropping_good` |
| U-12j | skipped があればコマンド失敗 | `main::reject_if_skipped_snapshots_*` |
| U-13a | path フィルタは成分単位 | `select::filter_matches_prefix_by_components` |
| U-13b | restore --path / --dry-run | `restore::dry_run_path_filter_*` / `path_filter_restores_*` |
| U-24 | CLI: 新コマンドと拒否条件 | `main::tests::*` |

既存の restore / store / background 系（U-01〜U-11, U-15〜U-31）も回帰対象。

## 5. 結合 E2E（I）重点

| ID | 内容 |
| --- | --- |
| I-02 ほか | 基本 snap / log / restore / verify / background |
| I-21 | 既定 snap が raw、`care` 後に zstd へ |
| I-22 | tree / find / restore --path / --dry-run |
| I-23 | init --config-only / --force |

## 6. 合格条件

- 単体: `cargo test` が pass
- E2E: `docs/run_e2e_tests.ps1` が pass
- 静的: `cargo clippy -- -D warnings`
