//! ============================================================================
//! 対象ディレクトリのスナップショット作成。
//! 対象ツリーは変更しない（読み取りのみ）。
//!
//! 方針: 確実な履歴バックアップ。明示除外以外は落とさない。
//! 除外は (1) 今開いているストア本体 (2) exclude_dir_names
//! (3) exclude_file_names / exclude_extensions (4) .snaplineignore の系統。
//! `.gitignore` は見ない。子階層の `.snapline` も自動除外しない。
//! 詳細は README の「スナップショットの包含・除外（重要）」を参照。
//! ファイル実体は object モジュールへ委譲し、ここでは目録作りに集中する。
//! ============================================================================

use std::{
    cell::{Cell, RefCell},
    fs::{self, Metadata},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use chrono::Utc;
use uuid::Uuid;
use walkdir::WalkDir;

use crate::{
    model::{Entry, EntryKind, FORMAT_VERSION, SnapshotManifest},
    object,
    pace::IoPace,
    progress::Progress,
    snaplineignore::SnaplineignoreMatcher,
    store::Store,
};

// ============================================================================
// スナップショット作成の結果。除外数も呼び出し側へ返す。
// ============================================================================
pub struct SnapshotOutcome {
    pub manifest: SnapshotManifest,

    /// 除外ルールによりスキップしたディレクトリ数（枝の根のみ。配下は数えない）。
    pub skipped_dirs: usize,
}

// ============================================================================
// 対象ツリーを走査し、設定に従い除外したうえでスナップショットを作る。
// CLI は create_with_pace を直接呼ぶ。本関数はテストと薄いラッパ用。
// ============================================================================
#[cfg_attr(not(test), allow(dead_code))]
pub fn create(store: &Store, message: Option<String>) -> Result<SnapshotOutcome> {
    create_with_pace(
        store,
        message,
        &mut crate::pace::IdlePace,
        &mut Progress::quiet(),
    )
}

// ============================================================================
// ペース制御付きスナップショット。通常経路は IdlePace、background のみ別実装。
// ============================================================================
pub fn create_with_pace(
    store: &Store,
    message: Option<String>,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<SnapshotOutcome> {
    let _lock = store.lock()?;
    if !store.config.target.is_dir() {
        bail!(
            "target directory does not exist: {}",
            store.config.target.display()
        );
    }

    let settings = store.config.settings.clone();
    let mut entries = Vec::new();
    let skipped_dirs = Cell::new(0_usize);
    let matcher = RefCell::new(SnaplineignoreMatcher::new(&store.config.target));
    let walk_error: RefCell<Option<anyhow::Error>> = RefCell::new(None);
    let mut entry_count = 0_usize;
    let mut file_count = 0_usize;

    progress.begin("Scanning files");

    // filter_entry(false) はその枝全体を降りない。
    // シンボリックリンクは追わない（リンク先ツリーを意図せず取り込みたくない）。
    let walker = WalkDir::new(&store.config.target)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|entry| {
            if walk_error.borrow().is_some() {
                return false;
            }
            if entry.depth() == 0 {
                return true;
            }

            // ストア本体（直下配置でも外部配置でも）は無条件に除外する。
            if entry.path() == store.root {
                if entry.file_type().is_dir() {
                    skipped_dirs.set(skipped_dirs.get() + 1);
                }
                return false;
            }

            let is_dir = entry.file_type().is_dir();

            // 既定のディレクトリ名除外。
            if is_dir && settings.should_exclude_dir_name(entry.file_name()) {
                skipped_dirs.set(skipped_dirs.get() + 1);
                return false;
            }

            // ファイル名・拡張子除外（ディレクトリには適用しない）。
            if !is_dir && settings.should_exclude_file(entry.path()) {
                return false;
            }

            // 各階層の `.snaplineignore` を重ねて判定する。
            match matcher.borrow_mut().should_exclude(entry.path(), is_dir) {
                Ok(true) => {
                    if is_dir {
                        skipped_dirs.set(skipped_dirs.get() + 1);
                    }
                    false
                }
                Ok(false) => true,
                Err(error) => {
                    *walk_error.borrow_mut() = Some(error);
                    false
                }
            }
        });

    for item in walker {
        if let Some(error) = walk_error.borrow_mut().take() {
            return Err(error);
        }
        let item = item.context("failed to traverse target directory")?;
        if item.depth() == 0 {
            continue;
        }

        let path = item.path();
        let relative = path
            .strip_prefix(&store.config.target)
            .context("entry escaped the target directory")?
            .to_path_buf();

        let metadata = fs::symlink_metadata(path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        let file_type = metadata.file_type();

        let (kind, object, symlink_target, symlink_is_dir) = if file_type.is_symlink() {
            let target = fs::read_link(path)
                .with_context(|| format!("failed to read symbolic link {}", path.display()))?;
            (
                EntryKind::Symlink,
                None,
                Some(target),
                fs::metadata(path)
                    .map(|followed| followed.is_dir())
                    .unwrap_or(false),
            )
        } else if file_type.is_dir() {
            (EntryKind::Directory, None, None, false)
        } else if file_type.is_file() {
            pace.before_entry()?;
            (
                EntryKind::File,
                Some(object::ingest_with_pace(store, path, &metadata, pace)?),
                None,
                false,
            )
        } else {
            bail!("unsupported filesystem entry: {}", path.display());
        };

        if matches!(kind, EntryKind::File) {
            file_count += 1;
        }
        entry_count += 1;
        progress.count(
            entry_count,
            &format!("entries ({file_count} files)"),
        );

        entries.push(Entry {
            path: relative,
            kind,
            object,
            size: metadata.len(),
            modified_unix_nanos: modified_nanos(&metadata),
            readonly: metadata.permissions().readonly(),
            symlink_target,
            symlink_is_dir,
        });
    }

    if let Some(error) = walk_error.into_inner() {
        return Err(error);
    }

    progress.done(&format!("{entry_count} entries ({file_count} files), done."));

    progress.begin("Writing snapshot");

    let manifest = SnapshotManifest {
        format_version: FORMAT_VERSION,
        id: format!(
            "{}-{}",
            Utc::now().format("%Y%m%dT%H%M%S%3fZ"),
            Uuid::new_v4().simple()
        ),
        created_at: Utc::now().to_rfc3339(),
        message,
        entries,
    };
    store.write_manifest(&manifest)?;
    progress.done("done.");

    Ok(SnapshotOutcome {
        manifest,
        skipped_dirs: skipped_dirs.get(),
    })
}

// ============================================================================
// 更新時刻をナノ秒へ正規化する。取得できない場合は None。
// ============================================================================
fn modified_nanos(metadata: &Metadata) -> Option<u128> {
    metadata
        .modified()
        .ok()?
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_nanos())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::create;
    use crate::store::Store;

    // ============================================================================
    // node_modules は除外し、.git と .gitignore は残ることを確認する。
    // ============================================================================
    #[test]
    fn excludes_named_dirs_but_keeps_git() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");

        fs::create_dir_all(target.join("project/.git/objects"))?;
        fs::create_dir_all(target.join("project/node_modules/pkg"))?;
        fs::create_dir_all(target.join("project/src"))?;
        fs::write(target.join("project/.git/HEAD"), "ref: refs/heads/main")?;
        fs::write(target.join("project/.gitignore"), "node_modules/")?;
        fs::write(target.join("project/node_modules/pkg/index.js"), "x")?;
        fs::write(target.join("project/src/main.rs"), "fn main() {}")?;

        let store = Store::init(&target, None)?;
        let outcome = create(&store, Some("test".into()))?;
        let paths: Vec<_> = outcome
            .manifest
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(paths.iter().any(|path| path.contains(".git/")));
        assert!(paths.iter().any(|path| path.ends_with(".gitignore")));
        assert!(paths.iter().any(|path| path.ends_with("src/main.rs")));
        assert!(!paths.iter().any(|path| path.contains("node_modules")));
        assert!(!paths.iter().any(|path| {
            path == ".snapline" || path.starts_with(".snapline/") || path.starts_with(".snapline\\")
        }));
        assert!(outcome.skipped_dirs >= 1);
        Ok(())
    }

    // ============================================================================
    // 子階層の `.snaplineignore` も適用されることを確認する。
    // ============================================================================
    #[test]
    fn applies_nested_snaplineignore_during_snapshot() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(target.join("app/cache"))?;
        fs::write(target.join(".snaplineignore"), "*.log\n")?;
        fs::write(target.join("app/.snaplineignore"), "cache/\n")?;
        fs::write(target.join("app/keep.txt"), "ok")?;
        fs::write(target.join("app/noise.log"), "x")?;
        fs::write(target.join("app/cache/x.bin"), "x")?;

        let store = Store::init(&target, None)?;
        let outcome = create(&store, None)?;
        let paths: Vec<_> = outcome
            .manifest
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(paths.iter().any(|path| path.ends_with("app/keep.txt")));
        assert!(paths.iter().any(|path| path.ends_with(".snaplineignore")));
        assert!(!paths.iter().any(|path| path.ends_with("noise.log")));
        assert!(!paths.iter().any(|path| path.contains("cache")));
        Ok(())
    }

    // ============================================================================
    // config のファイル名・拡張子除外がスナップショットに効くことを確認する。
    // ============================================================================
    #[test]
    fn excludes_configured_file_names_and_extensions() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(target.join("app"))?;
        fs::write(target.join("app/keep.txt"), "ok")?;
        fs::write(target.join("app/Thumbs.db"), "x")?;
        fs::write(target.join("app/noise.log"), "x")?;

        let store = Store::init(&target, None)?;
        let mut config = store.config.clone();
        config.settings.exclude_file_names = vec!["Thumbs.db".into()];
        config.settings.exclude_extensions = vec![".log".into()];
        crate::store::write_json_atomic(
            &store.root.join("config.json"),
            &config,
            &store.root.join("tmp"),
        )?;

        let store = Store::open(&target, None)?;
        let outcome = create(&store, None)?;
        let paths: Vec<_> = outcome
            .manifest
            .entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().replace('\\', "/"))
            .collect();

        assert!(paths.iter().any(|path| path.ends_with("app/keep.txt")));
        assert!(!paths.iter().any(|path| path.ends_with("Thumbs.db")));
        assert!(!paths.iter().any(|path| path.ends_with("noise.log")));
        Ok(())
    }
}
