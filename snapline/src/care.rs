//! ============================================================================
//! ストアの手入れ（整合確認のあと一括圧縮）。
//! 普段の snap とは分離し、たまにまとめて実行する想定。
//! ============================================================================

use std::collections::HashMap;

use anyhow::{Context, Result, bail};

use crate::{
    inspect,
    model::EntryKind,
    object::{self, CompactOutcome},
    pace::IoPace,
    progress::Progress,
    store::Store,
};

// ============================================================================
// care の結果。verify と compact の件数を返す。
// ============================================================================
#[derive(Debug)]
pub struct CareOutcome {
    pub snapshots: usize,
    pub objects: usize,
    pub compact: CompactOutcome,
    pub skipped: Vec<String>,
}

// ============================================================================
// 全スナップショットを検証したうえで、参照オブジェクトを圧縮する。
// ============================================================================
#[cfg_attr(not(test), allow(dead_code))]
pub fn care_with_pace(
    store: &Store,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<CareOutcome> {
    let _lock = store.lock()?;
    care_with_pace_locked(store, pace, progress)
}

// ============================================================================
// 書き込みロック取得済みの care。CLI は先に lock してから呼ぶ。
// ============================================================================
pub(crate) fn care_with_pace_locked(
    store: &Store,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<CareOutcome> {
    let listed = inspect::list_with_progress(store, progress)?;
    let manifests = listed.manifests;
    let skipped = listed.skipped;
    let mut objects = HashMap::new();

    progress.begin("Checking snapshots");
    for manifest in &manifests {
        for entry in &manifest.entries {
            if entry.kind == EntryKind::File {
                let hash = entry.object.as_deref().context("file object is missing")?;
                if let Some(previous_size) = objects.insert(hash.to_owned(), entry.size)
                    && previous_size != entry.size
                {
                    bail!("object has conflicting sizes in manifests: {hash}");
                }
            }
        }
    }
    progress.done(&format!("{} snapshots, done.", manifests.len()));

    let object_list: Vec<(String, u64)> = objects.into_iter().collect();
    let object_total = object_list.len();

    progress.begin("Verifying objects");
    for (index, (expected_hash, expected_size)) in object_list.iter().enumerate() {
        pace.before_entry()?;
        object::copy_verified_with_pace(
            store,
            expected_hash,
            *expected_size,
            std::io::sink(),
            pace,
        )?;
        progress.ratio(index + 1, object_total);
    }
    if object_total == 0 {
        progress.done("0 objects, done.");
    } else {
        progress.done(&format!("{object_total} objects, done."));
    }

    let compact = object::compact_referenced_with_pace(store, &object_list, pace, progress)?;

    Ok(CareOutcome {
        snapshots: manifests.len(),
        objects: object_total,
        compact,
        skipped,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::care_with_pace;
    use crate::{
        object::{CODEC_ZSTD, MAGIC},
        pace::IdlePace,
        progress::Progress,
        snapshot::{self, SnapshotOptions},
        store::Store,
    };

    // ============================================================================
    // care が verify 後に raw オブジェクトを圧縮することを確認する。
    // ============================================================================
    #[test]
    fn care_verifies_and_compacts_raw_objects() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), vec![b'x'; 64 * 1024])?;

        let store = Store::init(&target, None)?;
        snapshot::create_with_pace(
            &store,
            None,
            SnapshotOptions {
                reuse_unchanged: false,
                compress: false,
            },
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;

        let outcome = care_with_pace(&store, &mut IdlePace, &mut Progress::quiet())?;
        assert_eq!(outcome.snapshots, 1);
        assert!(outcome.compact.compressed >= 1);

        let hash = store
            .latest_manifest()?
            .expect("manifest")
            .entries
            .into_iter()
            .find_map(|entry| entry.object)
            .expect("object");
        let stored = fs::read(store.object_path(&hash)?)?;
        assert_eq!(&stored[..MAGIC.len()], MAGIC);
        assert_eq!(stored[MAGIC.len()], CODEC_ZSTD);
        Ok(())
    }

    // ============================================================================
    // 壊れたマニフェストがあっても読めた側は手入れし、skipped に残すことを確認する。
    // ============================================================================
    #[test]
    fn care_reports_skipped_broken_manifest_without_dropping_good() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), vec![b'x'; 64 * 1024])?;

        let store = Store::init(&target, None)?;
        snapshot::create_with_pace(
            &store,
            None,
            SnapshotOptions {
                reuse_unchanged: false,
                compress: false,
            },
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        fs::write(
            store.root.join("snapshots").join("zzzz-broken-care.json"),
            "{not-json",
        )?;

        let outcome = care_with_pace(&store, &mut IdlePace, &mut Progress::quiet())?;
        assert_eq!(outcome.snapshots, 1);
        assert!(outcome.compact.compressed >= 1);
        assert_eq!(outcome.skipped.len(), 1);
        assert!(outcome.skipped[0].contains("zzzz-broken-care"));
        Ok(())
    }

    // ============================================================================
    // オブジェクトが壊れていると care は圧縮へ進まず失敗することを確認する。
    // ============================================================================
    #[test]
    fn care_fails_when_object_is_corrupted() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), "hello")?;

        let store = Store::init(&target, None)?;
        let outcome = snapshot::create(&store, None)?;
        let hash = outcome
            .manifest
            .entries
            .iter()
            .find_map(|entry| entry.object.clone())
            .expect("object");
        let path = store.object_path(&hash)?;
        let mut bytes = fs::read(&path)?;
        let payload_at = MAGIC.len() + 1;
        assert!(bytes.len() > payload_at);
        bytes[payload_at] ^= 0xff;
        fs::write(&path, bytes)?;

        let error = care_with_pace(&store, &mut IdlePace, &mut Progress::quiet())
            .expect_err("care must fail on corrupt object");
        assert!(
            error.to_string().contains("integrity")
                || error
                    .chain()
                    .any(|cause| cause.to_string().contains("integrity")),
            "unexpected error: {error:#}"
        );
        Ok(())
    }
}
