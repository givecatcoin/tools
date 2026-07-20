//! ============================================================================
//! スナップショットからの安全な復元。
//! 復元先は新規または空ディレクトリのみ（既存ツリーを破壊しない）。
//! マニフェスト内の絶対パスや親参照を拒否する。
//! ファイルはハッシュ検証が通ってから最終パスへ公開する。
//! ============================================================================

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path},
};

use anyhow::{Context, Result, bail};
use filetime::FileTime;

use crate::{
    model::{Entry, EntryKind, FORMAT_VERSION, SnapshotManifest},
    object,
    pace::IoPace,
    store::Store,
};

// ============================================================================
// 指定スナップショットを destination へ復元し、エントリ数を返す。
// CLI は restore_with_pace を直接呼ぶ。本関数は薄いラッパ用。
// ============================================================================
#[allow(dead_code)]
pub fn restore(store: &Store, id: &str, destination: &Path) -> Result<usize> {
    restore_with_pace(store, id, destination, &mut crate::pace::IdlePace)
}

// ============================================================================
// ペース制御付き復元。通常経路は IdlePace、background のみ別実装。
// ============================================================================
pub fn restore_with_pace(
    store: &Store,
    id: &str,
    destination: &Path,
    pace: &mut dyn IoPace,
) -> Result<usize> {
    let manifest = store.read_manifest(id)?;
    validate_manifest(&manifest)?;
    prepare_destination(destination)?;

    for entry in entries_of_kind(&manifest, EntryKind::Directory) {
        fs::create_dir_all(destination.join(&entry.path))?;
    }
    for entry in entries_of_kind(&manifest, EntryKind::File) {
        restore_file(store, destination, entry, pace)?;
    }
    for entry in entries_of_kind(&manifest, EntryKind::Symlink) {
        restore_symlink(destination, entry)?;
    }

    for entry in manifest.entries.iter().rev() {
        apply_metadata(&destination.join(&entry.path), entry)?;
    }

    Ok(manifest.entries.len())
}

// ============================================================================
// マニフェストが復元に耐える形か検査する。
// verify からも使うため pub(crate)。
// ============================================================================
pub(crate) fn validate_manifest(manifest: &SnapshotManifest) -> Result<()> {
    if manifest.format_version != FORMAT_VERSION {
        bail!("unsupported snapshot format: {}", manifest.format_version);
    }

    let mut paths = HashSet::new();
    for entry in &manifest.entries {
        validate_relative_path(&entry.path)?;
        if !paths.insert(&entry.path) {
            bail!("duplicate path in manifest: {}", entry.path.display());
        }
        match entry.kind {
            EntryKind::File if entry.object.is_none() => {
                bail!("file has no object: {}", entry.path.display())
            }
            EntryKind::Symlink if entry.symlink_target.is_none() => {
                bail!("symbolic link has no target: {}", entry.path.display())
            }
            _ => {}
        }
    }
    Ok(())
}

// ============================================================================
// 相対パスとして安全か確認する。
// Normal 以外（ルート、親参照、カレント参照など）はすべて拒否する。
// ============================================================================
fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("manifest contains an empty path");
    }
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("unsafe path in manifest: {}", path.display());
    }
    Ok(())
}

// ============================================================================
// 復元先を用意する。既存の中身がある場合は拒否する。
// ============================================================================
fn prepare_destination(destination: &Path) -> Result<()> {
    if destination.exists() {
        if !destination.is_dir() {
            bail!("restore destination is not a directory");
        }
        if destination.read_dir()?.next().is_some() {
            bail!("restore destination must be empty");
        }
    } else {
        fs::create_dir_all(destination).with_context(|| {
            format!(
                "failed to create restore destination: {}",
                destination.display()
            )
        })?;
    }
    Ok(())
}

// ============================================================================
// 指定種別のエントリだけを取り出す。
// ============================================================================
fn entries_of_kind(manifest: &SnapshotManifest, kind: EntryKind) -> impl Iterator<Item = &Entry> {
    manifest
        .entries
        .iter()
        .filter(move |entry| entry.kind == kind)
}

// ============================================================================
// 1 ファイルを一時領域へ展開・検証してから最終パスへ置く。
// ============================================================================
fn restore_file(
    store: &Store,
    destination: &Path,
    entry: &Entry,
    pace: &mut dyn IoPace,
) -> Result<()> {
    let hash = entry.object.as_deref().context("file object is missing")?;
    let output_path = destination.join(&entry.path);
    let parent = output_path.parent().context("output path has no parent")?;
    fs::create_dir_all(parent)?;

    pace.before_entry()?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    object::copy_verified_with_pace(store, hash, entry.size, &mut temp, pace)?;
    temp.as_file().sync_all()?;
    temp.persist_noclobber(&output_path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to restore {}", output_path.display()))?;
    Ok(())
}

// ============================================================================
// 更新時刻と読み取り専用フラグを戻す。
// ============================================================================
fn apply_metadata(path: &Path, entry: &Entry) -> Result<()> {
    // シンボリックリンクのメタデータ操作は OS 差が大きく、ここでは触らない。
    if entry.kind == EntryKind::Symlink {
        return Ok(());
    }

    if let Some(nanos) = entry.modified_unix_nanos {
        let seconds = i64::try_from(nanos / 1_000_000_000)
            .context("modification timestamp is out of range")?;
        let subsecond_nanos = u32::try_from(nanos % 1_000_000_000)?;
        filetime::set_file_mtime(path, FileTime::from_unix_time(seconds, subsecond_nanos))?;
    }

    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(entry.readonly);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(unix)]
// ============================================================================
// UNIX 向けシンボリックリンク復元。
// ============================================================================
fn restore_symlink(destination: &Path, entry: &Entry) -> Result<()> {
    use std::os::unix::fs::symlink;

    let output = destination.join(&entry.path);
    fs::create_dir_all(output.parent().context("link path has no parent")?)?;
    symlink(
        entry
            .symlink_target
            .as_ref()
            .context("link target is missing")?,
        &output,
    )
    .with_context(|| format!("failed to create symbolic link {}", output.display()))
}

#[cfg(windows)]
// ============================================================================
// Windows 向けシンボリックリンク復元。
// dir/file API をエントリ情報に応じて切り替える。
// ============================================================================
fn restore_symlink(destination: &Path, entry: &Entry) -> Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let output = destination.join(&entry.path);
    fs::create_dir_all(output.parent().context("link path has no parent")?)?;
    let target = entry
        .symlink_target
        .as_ref()
        .context("link target is missing")?;

    // Windows はリンク種別に dir/file API の区別がある。
    let result = if entry.symlink_is_dir {
        symlink_dir(target, &output)
    } else {
        symlink_file(target, &output)
    };
    result.with_context(|| {
        format!(
            "failed to create symbolic link {}; enable Windows Developer Mode if needed",
            output.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::validate_relative_path;

    // ============================================================================
    // 通常の相対パスを受け入れることを確認する。
    // ============================================================================
    #[test]
    fn accepts_normal_relative_path() {
        assert!(validate_relative_path(Path::new("project/src/main.rs")).is_ok());
    }

    // ============================================================================
    // 親ディレクトリ参照を拒否することを確認する。
    // ============================================================================
    #[test]
    fn rejects_parent_traversal() {
        assert!(validate_relative_path(Path::new("../outside")).is_err());
    }

    // ============================================================================
    // 絶対パスを拒否することを確認する。
    // ============================================================================
    #[test]
    fn rejects_absolute_path() {
        assert!(validate_relative_path(Path::new("/outside")).is_err());
    }
}
