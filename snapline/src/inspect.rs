//! ============================================================================
//! 履歴の一覧と整合性検証。
//! 書き込み系（snapshot）とは切り離し、読み取り専用の観測操作をまとめる。
//! ============================================================================

use std::{collections::HashMap, fmt};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde::de::{Deserializer, IgnoredAny, SeqAccess, Visitor};

use crate::{
    model::{EntryKind, SnapshotManifest, SnapshotSummary},
    object,
    pace::IoPace,
    progress::Progress,
    restore::validate_manifest,
    store::{Store, read_json},
};

// ============================================================================
// `snapline log` 用の要約行。
// ============================================================================
#[derive(Debug, Clone)]
pub struct SnapshotLogRow {
    pub id: String,
    pub created_at: String,
    pub message: Option<String>,
    pub entry_count: usize,
}

impl From<SnapshotSummary> for SnapshotLogRow {
    fn from(summary: SnapshotSummary) -> Self {
        Self {
            id: summary.id,
            created_at: summary.created_at,
            message: summary.message,
            entry_count: summary.entry_count,
        }
    }
}

// ============================================================================
// 旧ストア互換のため、マニフェストから件数だけ読む中間形。
// ============================================================================
#[derive(Deserialize)]
struct SnapshotLogFile {
    id: String,
    created_at: String,
    message: Option<String>,
    #[serde(default, deserialize_with = "deserialize_entry_count")]
    entries: usize,
}

// ============================================================================
// 進捗付きでマニフェスト全体を読む（verify 向け）。
// ============================================================================
pub fn list_with_progress(
    store: &Store,
    progress: &mut Progress,
) -> Result<Vec<SnapshotManifest>> {
    let paths = store.snapshot_manifest_paths()?;
    progress.begin("Reading snapshots");
    let mut manifests = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let label = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("snapshot");
        progress.step(index + 1, paths.len(), &short_id(label));
        manifests.push(read_json(path)?);
    }
    if paths.is_empty() {
        progress.done("0 snapshots, done.");
    } else {
        progress.done(&format!("{} snapshots, done.", manifests.len()));
    }
    manifests.sort_by(|left: &SnapshotManifest, right| left.created_at.cmp(&right.created_at));
    Ok(manifests)
}

// ============================================================================
// `log` 用。要約ファイルを読む。無ければ旧マニフェストから自動生成する。
// `newest` が Some(n) のときは新しい順に最大 n 件（表示は古い→新しい）。
// ============================================================================
pub fn list_log_rows(
    store: &Store,
    progress: &mut Progress,
    newest: Option<usize>,
) -> Result<Vec<SnapshotLogRow>> {
    ensure_summaries(store, progress)?;

    let paths = store.snapshot_manifest_paths()?;
    progress.begin("Reading snapshot summaries");
    let mut rows = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let label = path
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("snapshot");
        progress.step(index + 1, paths.len(), &short_id(label));
        let summary = store.read_summary(label)?;
        rows.push(SnapshotLogRow::from(summary));
    }
    if paths.is_empty() {
        progress.done("0 snapshots, done.");
    } else {
        progress.done(&format!("{} snapshots, done.", rows.len()));
    }

    rows.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    if let Some(limit) = newest {
        let start = rows.len().saturating_sub(limit);
        rows = rows.split_off(start);
    }
    Ok(rows)
}

// ============================================================================
// 欠落している要約をマニフェストから補完する（旧ストア互換）。
// 生成した件数を返す。
// ============================================================================
pub fn ensure_summaries(store: &Store, progress: &mut Progress) -> Result<usize> {
    let paths = store.snapshot_manifest_paths()?;
    let mut missing = Vec::new();
    for path in &paths {
        let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
            continue;
        };
        if !summary_is_ready(store, id)? {
            missing.push((path.clone(), id.to_owned()));
        }
    }
    if missing.is_empty() {
        return Ok(0);
    }

    // 要約書き込みはストア変更なので排他する。
    let _lock = store.lock()?;
    progress.begin("Building snapshot summaries");
    let mut created = 0_usize;
    for (index, (path, id)) in missing.iter().enumerate() {
        progress.step(index + 1, missing.len(), &short_id(id));
        // ロック待ちのあいだに他プロセスが書いた場合はスキップする。
        if summary_is_ready(store, id)? {
            continue;
        }
        let file: SnapshotLogFile = read_json(path)?;
        let summary = SnapshotSummary {
            format_version: crate::model::FORMAT_VERSION,
            id: file.id,
            created_at: file.created_at,
            message: file.message,
            entry_count: file.entries,
        };
        if summary.id != *id {
            bail!(
                "snapshot file name does not match id field: {} vs {}",
                id,
                summary.id
            );
        }
        store.write_summary(&summary)?;
        created += 1;
    }
    progress.done(&format!("{created} summaries created, done."));
    Ok(created)
}

// ============================================================================
// 全マニフェストと参照オブジェクトを検証する。
// 戻り値は (スナップショット数, ユニークオブジェクト数)。
// ============================================================================
#[allow(dead_code)]
pub fn verify(store: &Store, progress: &mut Progress) -> Result<(usize, usize)> {
    verify_with_pace(store, progress, &mut crate::pace::IdlePace)
}

// ============================================================================
// ペース制御付き検証。通常経路は IdlePace、background のみ別実装。
// ============================================================================
pub fn verify_with_pace(
    store: &Store,
    progress: &mut Progress,
    pace: &mut dyn IoPace,
) -> Result<(usize, usize)> {
    let manifests = list_with_progress(store, progress)?;

    // 同じオブジェクトが複数スナップショットから指されるのは正常。
    // ただし「同じハッシュなのにサイズが違う」は壊れているので拒否する。
    let mut objects = HashMap::new();

    progress.begin("Checking snapshots");
    for manifest in &manifests {
        validate_manifest(manifest)
            .with_context(|| format!("invalid snapshot manifest: {}", manifest.id))?;
        for entry in &manifest.entries {
            if entry.kind == EntryKind::File {
                let hash = entry.object.as_deref().context("file object is missing")?;
                if let Some(previous_size) = objects.insert(hash, entry.size)
                    && previous_size != entry.size
                {
                    bail!("object has conflicting sizes in manifests: {hash}");
                }
            }
        }
    }
    progress.done(&format!("{} snapshots, done.", manifests.len()));

    let object_total = objects.len();
    progress.begin("Verifying objects");

    // 実体は展開しながらハッシュ照合する。圧縮破損もここで検知できる。
    for (index, (expected_hash, expected_size)) in objects.iter().enumerate() {
        pace.before_entry()?;
        object::copy_verified_with_pace(store, expected_hash, *expected_size, std::io::sink(), pace)?;
        progress.ratio(index + 1, object_total);
    }
    if object_total == 0 {
        progress.done("0 objects, done.");
    }

    Ok((manifests.len(), objects.len()))
}

fn short_id(id: &str) -> String {
    id.chars().take(12).collect()
}

// ============================================================================
// 要約が存在し、ファイル名と id が一致することを確認する。
// ============================================================================
fn summary_is_ready(store: &Store, id: &str) -> Result<bool> {
    if !store.summary_path(id)?.is_file() {
        return Ok(false);
    }
    match store.read_summary(id) {
        Ok(summary) => Ok(summary.id == id),
        Err(_) => Ok(false),
    }
}

fn deserialize_entry_count<'de, D>(deserializer: D) -> Result<usize, D::Error>
where
    D: Deserializer<'de>,
{
    struct CountVisitor;

    impl<'de> Visitor<'de> for CountVisitor {
        type Value = usize;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an array of snapshot entries")
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<usize, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut count = 0;
            while seq.next_element::<IgnoredAny>()?.is_some() {
                count += 1;
            }
            Ok(count)
        }
    }

    deserializer.deserialize_seq(CountVisitor)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::{SnapshotLogFile, ensure_summaries, list_log_rows};
    use crate::{
        model::{Entry, EntryKind, FORMAT_VERSION, SnapshotManifest},
        progress::Progress,
        store::Store,
    };

    // ============================================================================
    // entries 配列を要素型へ落とさず件数だけ取れることを確認する。
    // ============================================================================
    #[test]
    fn counts_entries_without_materializing_them() {
        let json = r#"{
            "id": "abc",
            "created_at": "2024-01-01T00:00:00Z",
            "message": "hi",
            "entries": [{"path":"a"},{"path":"b"},{"path":"c"}]
        }"#;
        let file: SnapshotLogFile = serde_json::from_str(json).unwrap();
        assert_eq!(file.id, "abc");
        assert_eq!(file.entries, 3);
        assert_eq!(file.message.as_deref(), Some("hi"));
    }

    // ============================================================================
    // snapshot 書き込み時に要約が同時保存されることを確認する。
    // ============================================================================
    #[test]
    fn write_manifest_also_writes_summary() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("tree");
        fs::create_dir_all(&target)?;
        let store = Store::init(&target, None)?;

        let manifest = sample_manifest(
            "20240101T000000000Z-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "2024-01-01T00:00:00Z",
            2,
        );
        store.write_manifest(&manifest)?;

        let summary = store.read_summary(&manifest.id)?;
        assert_eq!(summary.id, manifest.id);
        assert_eq!(summary.entry_count, 2);
        assert_eq!(summary.message.as_deref(), Some("hello"));
        Ok(())
    }

    // ============================================================================
    // 旧ストア（要約なし）で log すると自動生成されることを確認する。
    // ============================================================================
    #[test]
    fn migrates_missing_summaries_on_log() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("tree");
        fs::create_dir_all(&target)?;
        let store = Store::init(&target, None)?;

        let id = "20240101T000000000Z-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let manifest = sample_manifest(id, "2024-01-01T00:00:00Z", 3);
        // 旧版相当: マニフェストだけ書き、要約は置かない。
        crate::store::write_json_atomic(
            &store.manifest_path(id)?,
            &manifest,
            &store.root.join("tmp"),
        )?;
        assert!(!store.summary_path(id)?.is_file());

        let mut progress = Progress::quiet();
        let created = ensure_summaries(&store, &mut progress)?;
        assert_eq!(created, 1);
        let summary = store.read_summary(id)?;
        assert_eq!(summary.entry_count, 3);

        let rows = list_log_rows(&store, &mut progress, None)?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entry_count, 3);
        Ok(())
    }

    // ============================================================================
    // newest 指定で末尾 N 件だけ返すことを確認する。
    // ============================================================================
    #[test]
    fn list_log_rows_respects_newest_limit() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("tree");
        fs::create_dir_all(&target)?;
        let store = Store::init(&target, None)?;

        let older = sample_manifest(
            "20240101T000000000Z-cccccccccccccccccccccccccccccccc",
            "2024-01-01T00:00:00Z",
            1,
        );
        let newer = sample_manifest(
            "20240102T000000000Z-dddddddddddddddddddddddddddddddd",
            "2024-01-02T00:00:00Z",
            2,
        );
        store.write_manifest(&older)?;
        store.write_manifest(&newer)?;

        let mut progress = Progress::quiet();
        let rows = list_log_rows(&store, &mut progress, Some(1))?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, newer.id);
        assert_eq!(rows[0].entry_count, 2);
        Ok(())
    }

    fn sample_manifest(id: &str, created_at: &str, entry_count: usize) -> SnapshotManifest {
        let entries = (0..entry_count)
            .map(|index| Entry {
                path: format!("f{index}.txt").into(),
                kind: EntryKind::File,
                object: Some(format!("{index:064x}")),
                size: 1,
                modified_unix_nanos: None,
                readonly: false,
                symlink_target: None,
                symlink_is_dir: false,
            })
            .collect();
        SnapshotManifest {
            format_version: FORMAT_VERSION,
            id: id.to_owned(),
            created_at: created_at.to_owned(),
            message: Some("hello".into()),
            entries,
        }
    }
}
