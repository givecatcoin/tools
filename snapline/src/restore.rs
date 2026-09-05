//! ============================================================================
//! スナップショットからの安全な復元。
//! 復元先は新規または空ディレクトリのみ（既存ツリーを破壊しない）。
//! `--path` で一部だけ展開できる。上書き復元はしない。
//! ファイルはハッシュ検証が通ってから最終パスへ公開する。
//! ============================================================================

use std::{
    collections::HashSet,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use filetime::FileTime;

use crate::{
    model::{Entry, EntryKind, FORMAT_VERSION, SnapshotManifest},
    object,
    pace::IoPace,
    progress::Progress,
    select::{PathFilter, SelectionSummary, format_bytes, select_entries},
    store::Store,
};

// ============================================================================
// restore のオプション。
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct RestoreOptions {
    /// スナップ内の相対パス。指定時はその配下だけを展開する。
    pub path: Option<String>,
    /// 書き込まず、想定件数と容量だけ出す。
    pub dry_run: bool,
}

// ============================================================================
// restore の結果（実書き込みまたは dry-run）。
// ============================================================================
#[derive(Debug, Clone)]
pub struct RestoreOutcome {
    pub summary: SelectionSummary,
    pub dry_run: bool,
    pub filter: Option<String>,
    pub destination: PathBuf,
}

// ============================================================================
// 指定スナップショットを destination へ復元する。
// ============================================================================
#[allow(dead_code)]
pub fn restore(
    store: &Store,
    id: &str,
    destination: &Path,
    options: RestoreOptions,
) -> Result<RestoreOutcome> {
    restore_with_pace(
        store,
        id,
        destination,
        options,
        &mut crate::pace::IdlePace,
        &mut Progress::quiet(),
    )
}

// ============================================================================
// ペース制御付き復元。通常経路は IdlePace、background のみ別実装。
// ============================================================================
pub fn restore_with_pace(
    store: &Store,
    id: &str,
    destination: &Path,
    options: RestoreOptions,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<RestoreOutcome> {
    progress.begin("Reading snapshot");
    let manifest = store.read_manifest(id)?;
    validate_manifest(&manifest)?;
    progress.done(&format!("{} entries in manifest, done.", manifest.entries.len()));

    progress.begin("Planning restore");
    let filter = PathFilter::parse(options.path.as_deref())?;
    let selected = select_entries(&manifest, &filter)?;
    let summary = SelectionSummary::from_entries(selected.iter().copied());
    progress.done(&format!(
        "{} entries ({} files, {}), done.",
        summary.entry_count,
        summary.file_count,
        format_bytes(summary.file_bytes)
    ));

    if options.dry_run {
        progress.begin("Checking destination");
        check_destination(destination)?;
        progress.done("ok, dry-run (no files written).");
        return Ok(RestoreOutcome {
            summary,
            dry_run: true,
            filter: filter.display(),
            destination: destination.to_path_buf(),
        });
    }

    progress.begin("Preparing destination");
    prepare_destination(destination)?;
    progress.done("done.");

    let dir_entries: Vec<_> = selected
        .iter()
        .copied()
        .filter(|entry| entry.kind == EntryKind::Directory)
        .collect();
    let file_entries: Vec<_> = selected
        .iter()
        .copied()
        .filter(|entry| entry.kind == EntryKind::File)
        .collect();
    let symlink_entries: Vec<_> = selected
        .iter()
        .copied()
        .filter(|entry| entry.kind == EntryKind::Symlink)
        .collect();

    progress.begin("Creating directories");
    for (index, entry) in dir_entries.iter().enumerate() {
        fs::create_dir_all(destination.join(&entry.path))?;
        progress.ratio(index + 1, dir_entries.len().max(1));
    }
    if dir_entries.is_empty() {
        progress.done("0 directories, done.");
    }

    progress.begin("Restoring files");
    for (index, entry) in file_entries.iter().enumerate() {
        restore_file(store, destination, entry, pace)?;
        progress.ratio(index + 1, file_entries.len().max(1));
    }
    if file_entries.is_empty() {
        progress.done("0 files, done.");
    }

    progress.begin("Restoring symlinks");
    for (index, entry) in symlink_entries.iter().enumerate() {
        restore_symlink(destination, entry)?;
        progress.ratio(index + 1, symlink_entries.len().max(1));
    }
    if symlink_entries.is_empty() {
        progress.done("0 symlinks, done.");
    }

    progress.begin("Applying metadata");
    let total_entries = selected.len().max(1);
    for (index, entry) in selected.iter().rev().enumerate() {
        apply_metadata(&destination.join(&entry.path), entry)?;
        progress.ratio(index + 1, total_entries);
    }
    if selected.is_empty() {
        progress.done("0 entries, done.");
    }

    Ok(RestoreOutcome {
        summary,
        dry_run: false,
        filter: filter.display(),
        destination: destination.to_path_buf(),
    })
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
// 復元先の条件だけ確認する（作成しない）。dry-run 用。
// ============================================================================
fn check_destination(destination: &Path) -> Result<()> {
    if destination.exists() {
        if !destination.is_dir() {
            bail!("restore destination is not a directory");
        }
        if destination.read_dir()?.next().is_some() {
            bail!("restore destination must be empty");
        }
    }
    Ok(())
}

// ============================================================================
// 復元先を用意する。既存の中身がある場合は拒否する。
// ============================================================================
fn prepare_destination(destination: &Path) -> Result<()> {
    check_destination(destination)?;
    if !destination.exists() {
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
fn restore_symlink(destination: &Path, entry: &Entry) -> Result<()> {
    use std::os::windows::fs::{symlink_dir, symlink_file};

    let output = destination.join(&entry.path);
    fs::create_dir_all(output.parent().context("link path has no parent")?)?;
    let target = entry
        .symlink_target
        .as_ref()
        .context("link target is missing")?;

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
    use std::{fs, path::Path};

    use anyhow::Result;

    use super::{RestoreOptions, restore, validate_relative_path};
    use crate::store::Store;

    #[test]
    fn accepts_normal_relative_path() {
        assert!(validate_relative_path(Path::new("project/src/main.rs")).is_ok());
    }

    #[test]
    fn rejects_parent_traversal() {
        assert!(validate_relative_path(Path::new("../outside")).is_err());
    }

    #[test]
    fn rejects_absolute_path() {
        assert!(validate_relative_path(Path::new("/outside")).is_err());
    }

    // ============================================================================
    // --path 付き dry-run が想定容量を出し、宛先を作らないことを確認する。
    // ============================================================================
    #[test]
    fn dry_run_path_filter_reports_size_without_writing() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(target.join("docs"))?;
        fs::create_dir_all(target.join("src"))?;
        fs::write(target.join("docs/a.txt"), "hello-docs")?;
        fs::write(target.join("src/b.txt"), "hello-src")?;
        let store = Store::init(&target, None)?;
        crate::snapshot::create(&store, Some("base".into()))?;

        let dest = root.path().join("out");
        let outcome = restore(
            &store,
            store
                .latest_manifest()?
                .expect("manifest")
                .id
                .as_str(),
            &dest,
            RestoreOptions {
                path: Some("docs".into()),
                dry_run: true,
            },
        )?;
        assert!(outcome.dry_run);
        assert!(outcome.summary.file_bytes >= 10);
        assert!(!dest.exists());
        assert_eq!(outcome.filter.as_deref(), Some("docs"));
        Ok(())
    }

    // ============================================================================
    // --path 付き復元が指定配下だけを展開することを確認する。
    // ============================================================================
    #[test]
    fn path_filter_restores_only_matching_subtree() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(target.join("docs"))?;
        fs::create_dir_all(target.join("src"))?;
        fs::write(target.join("docs/a.txt"), "hello-docs")?;
        fs::write(target.join("src/b.txt"), "hello-src")?;
        let store = Store::init(&target, None)?;
        let id = crate::snapshot::create(&store, Some("base".into()))?.manifest.id;

        let dest = root.path().join("out");
        let outcome = restore(
            &store,
            &id,
            &dest,
            RestoreOptions {
                path: Some("docs".into()),
                dry_run: false,
            },
        )?;
        assert!(!outcome.dry_run);
        assert!(dest.join("docs/a.txt").is_file());
        assert!(!dest.join("src/b.txt").exists());
        Ok(())
    }
}
