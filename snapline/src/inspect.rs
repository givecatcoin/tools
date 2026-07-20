//! ============================================================================
//! 履歴の一覧と整合性検証。
//! 書き込み系（snapshot）とは切り離し、読み取り専用の観測操作をまとめる。
//! ============================================================================

use std::{
    collections::HashMap,
    fs,
    io::{self, Write},
};

use anyhow::{Context, Result, bail};

use crate::{
    model::{EntryKind, SnapshotManifest},
    object,
    pace::IoPace,
    restore::validate_manifest,
    store::{Store, read_json},
};

// ============================================================================
// ストア内のスナップショットを古い順に返す。
// ============================================================================
pub fn list(store: &Store) -> Result<Vec<SnapshotManifest>> {
    let mut manifests = Vec::new();
    for item in fs::read_dir(store.root.join("snapshots"))? {
        let path = item?.path();

        // 拡張子で目録だけ拾う。将来別形式を置いても誤読しにくくする。
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }

        manifests.push(read_json(&path)?);
    }

    manifests.sort_by(|left: &SnapshotManifest, right| left.created_at.cmp(&right.created_at));
    Ok(manifests)
}

// ============================================================================
// 全マニフェストと参照オブジェクトを検証する。
// 戻り値は (スナップショット数, ユニークオブジェクト数)。
// ============================================================================
pub fn verify(store: &Store, progress: impl Write) -> Result<(usize, usize)> {
    verify_with_pace(store, progress, &mut crate::pace::IdlePace)
}

// ============================================================================
// ペース制御付き検証。通常経路は IdlePace、background のみ別実装。
// ============================================================================
pub fn verify_with_pace(
    store: &Store,
    mut progress: impl Write,
    pace: &mut dyn IoPace,
) -> Result<(usize, usize)> {
    let manifests = list(store)?;

    // 同じオブジェクトが複数スナップショットから指されるのは正常。
    // ただし「同じハッシュなのにサイズが違う」は壊れているので拒否する。
    let mut objects = HashMap::new();

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

    // 実体は展開しながらハッシュ照合する。圧縮破損もここで検知できる。
    for (index, (expected_hash, expected_size)) in objects.iter().enumerate() {
        pace.before_entry()?;
        object::copy_verified_with_pace(store, expected_hash, *expected_size, io::sink(), pace)?;
        if (index + 1) % 100 == 0 {
            writeln!(progress, "verified {} objects", index + 1)?;
        }
    }

    Ok((manifests.len(), objects.len()))
}
