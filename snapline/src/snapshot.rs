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
    collections::HashMap,
    fs::{self, Metadata},
    path::{Path, PathBuf},
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
// スナップショット作成時のオプション。
// 包含・除外ルールは変えない。既定は普段使い（簡易整合・無圧縮）。
// ============================================================================
#[derive(Debug, Clone, Copy)]
pub struct SnapshotOptions {
    /// 直前スナップショットと size/mtime が一致し、オブジェクト簡易整合が通れば
    /// 内容の再読込を省略してハッシュを再利用する。
    pub reuse_unchanged: bool,

    /// true なら取り込み時に zstd を試す（`snap --compress`）。普段は false。
    pub compress: bool,
}

impl Default for SnapshotOptions {
    fn default() -> Self {
        Self::everyday()
    }
}

impl SnapshotOptions {
    // ========================================================================
    // 普段の snap: 簡易整合あり・無圧縮。
    // ========================================================================
    pub fn everyday() -> Self {
        Self {
            reuse_unchanged: true,
            compress: false,
        }
    }

    // ========================================================================
    // CLI フラグからオプションを組み立てる。
    // ========================================================================
    pub fn from_flags(rehash: bool, compress: bool) -> Self {
        Self {
            reuse_unchanged: !rehash,
            compress,
        }
    }
}

// ============================================================================
// スナップショット作成の結果。除外数も呼び出し側へ返す。
// ============================================================================
pub struct SnapshotOutcome {
    pub manifest: SnapshotManifest,

    /// 除外ルールによりスキップしたディレクトリ数（枝の根のみ。配下は数えない）。
    pub skipped_dirs: usize,

    /// 簡易整合により内容取り込みを省略したファイル数。
    pub reused_files: usize,

    /// 今回新たに取り込んだファイルの論理サイズ合計（reuse 以外）。
    pub ingested_bytes: u64,
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
        SnapshotOptions::default(),
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
    options: SnapshotOptions,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<SnapshotOutcome> {
    let _lock = store.lock()?;
    create_with_pace_locked(store, message, options, pace, progress)
}

// ============================================================================
// 書き込みロック取得済みのスナップショット。CLI は先に lock してから呼ぶ。
// ============================================================================
pub(crate) fn create_with_pace_locked(
    store: &Store,
    message: Option<String>,
    options: SnapshotOptions,
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<SnapshotOutcome> {
    if !store.config.target.is_dir() {
        bail!(
            "target directory does not exist: {}",
            store.config.target.display()
        );
    }

    let settings = store.config.settings.clone();

    // reuse 用の直前マニフェスト読込は件数が増えると重いので、無表示にしない。
    progress.begin("Loading previous snapshot");
    let previous_files = if options.reuse_unchanged {
        load_previous_files(store)?
    } else {
        HashMap::new()
    };
    progress.done(&format!(
        "{} reusable paths, done.",
        previous_files.len()
    ));

    let mut entries = Vec::new();
    let skipped_dirs = Cell::new(0_usize);
    let matcher = RefCell::new(SnaplineignoreMatcher::new(&store.config.target));
    let walk_error: RefCell<Option<anyhow::Error>> = RefCell::new(None);
    let mut entry_count = 0_usize;
    let mut file_count = 0_usize;
    let mut reused_files = 0_usize;
    let mut ingested_bytes = 0_u64;

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
            let hash = if let Some(reused) =
                try_reuse_unchanged(store, &relative, &metadata, &previous_files)?
            {
                reused_files += 1;
                reused
            } else {
                let mode = if options.compress {
                    object::IngestMode::Compress
                } else {
                    object::IngestMode::Raw
                };
                let hash = object::ingest_with_pace(store, path, &metadata, mode, pace)?;
                ingested_bytes = ingested_bytes.saturating_add(metadata.len());
                hash
            };
            (EntryKind::File, Some(hash), None, false)
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
        reused_files,
        ingested_bytes,
    })
}

// ============================================================================
// 直前スナップショットのファイルエントリを path → 参照用データへ載せる。
// ============================================================================
fn load_previous_files(store: &Store) -> Result<HashMap<PathBuf, PreviousFile>> {
    let Some(manifest) = store.latest_manifest()? else {
        return Ok(HashMap::new());
    };

    let mut map = HashMap::new();
    for entry in manifest.entries {
        if entry.kind != EntryKind::File {
            continue;
        }
        let Some(object) = entry.object else {
            continue;
        };
        let Some(modified_unix_nanos) = entry.modified_unix_nanos else {
            continue;
        };
        map.insert(
            entry.path,
            PreviousFile {
                object,
                size: entry.size,
                modified_unix_nanos,
            },
        );
    }
    Ok(map)
}

// ============================================================================
// 簡易整合が通れば旧ハッシュを返す。通らなければ None（フル取り込みへ）。
//
// 条件:
// 1. 直前に同パスのファイルがある
// 2. size と mtime（双方 Some）が一致
// 3. オブジェクトが存在しヘッダが読める
//
// 内容バイトは見ない。mtime が変わらない改変は検出できない（Git の stat 照合と同型の限界）。
// ============================================================================
fn try_reuse_unchanged(
    store: &Store,
    relative: &Path,
    metadata: &Metadata,
    previous_files: &HashMap<PathBuf, PreviousFile>,
) -> Result<Option<String>> {
    let Some(previous) = previous_files.get(relative) else {
        return Ok(None);
    };
    let Some(current_mtime) = modified_nanos(metadata) else {
        return Ok(None);
    };
    if previous.size != metadata.len() || previous.modified_unix_nanos != current_mtime {
        return Ok(None);
    }
    if !object::looks_consistent(store, &previous.object)? {
        return Ok(None);
    }
    Ok(Some(previous.object.clone()))
}

// ============================================================================
// 増分再利用の照合に使う、直前スナップショット上のファイル情報。
// ============================================================================
struct PreviousFile {
    object: String,
    size: u64,
    modified_unix_nanos: u128,
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
    use std::{
        fs,
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use anyhow::Result;

    use super::{SnapshotOptions, create, create_with_pace};
    use crate::{pace::IdlePace, progress::Progress, store::Store};

    // ============================================================================
    // reuse 準備の進捗が走査より先に出ることを確認する。
    // ============================================================================
    #[test]
    fn reports_loading_previous_before_scanning() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("a.txt"), "one")?;
        let store = Store::init(&target, None)?;
        create(&store, Some("first".into()))?;

        let buffer = Arc::new(Mutex::new(Vec::new()));
        let capture = buffer.clone();
        let mut progress = Progress::for_tests(ProgressCapture { inner: capture });
        create_with_pace(
            &store,
            Some("second".into()),
            SnapshotOptions::from_flags(false, false),
            &mut IdlePace,
            &mut progress,
        )?;

        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        let load_at = output
            .find("Loading previous snapshot")
            .expect("must announce previous-snapshot load");
        let scan_at = output
            .find("Scanning files")
            .expect("must announce scanning");
        assert!(
            load_at < scan_at,
            "loading progress must appear before scanning:\n{output}"
        );
        assert!(output.contains("reusable paths"));
        Ok(())
    }

    struct ProgressCapture {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for ProgressCapture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

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

    // ============================================================================
    // 既定の普段使いでは、size/mtime 一致＋オブジェクト存在なら取り込みを省略する。
    // ============================================================================
    #[test]
    fn reuses_unchanged_files_with_simple_consistency_check() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        let file = target.join("note.txt");
        fs::write(&file, "hello")?;

        let store = Store::init(&target, None)?;
        let first = create(&store, Some("first".into()))?;
        let first_hash = first
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");

        let second = create(&store, Some("second".into()))?;
        let second_hash = second
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");

        assert_eq!(first_hash, second_hash);
        assert!(second.reused_files >= 1);
        assert_eq!(second.ingested_bytes, 0);
        Ok(())
    }

    // ============================================================================
    // オブジェクトが欠けると簡易整合が失敗し、再取り込みになることを確認する。
    // ============================================================================
    #[test]
    fn falls_back_to_ingest_when_object_is_missing() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), "hello")?;

        let store = Store::init(&target, None)?;
        let first = create(&store, None)?;
        let hash = first
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");
        fs::remove_file(store.object_path(&hash)?)?;

        let second = create(&store, None)?;

        assert_eq!(second.reused_files, 0);
        assert!(store.object_path(&hash)?.is_file());
        Ok(())
    }

    // ============================================================================
    // --rehash では再利用せず raw のまま取り込むことを確認する。
    // ============================================================================
    #[test]
    fn rehash_reads_all_without_reuse() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), vec![b'a'; 64 * 1024])?;

        let store = Store::init(&target, None)?;
        create(&store, None)?;
        let second = create_with_pace(
            &store,
            Some("rehash".into()),
            SnapshotOptions::from_flags(true, false),
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        assert_eq!(second.reused_files, 0);

        let hash = second
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");
        let stored = fs::read(store.object_path(&hash)?)?;
        assert_eq!(&stored[..crate::object::MAGIC.len()], crate::object::MAGIC);
        assert_eq!(stored[crate::object::MAGIC.len()], crate::object::CODEC_RAW);
        Ok(())
    }

    // ============================================================================
    // --rehash --compress では全読込のうえ圧縮することを確認する。
    // ============================================================================
    #[test]
    fn rehash_with_compress_stores_zstd() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), vec![b'a'; 64 * 1024])?;

        let store = Store::init(&target, None)?;
        let outcome = create_with_pace(
            &store,
            Some("both".into()),
            SnapshotOptions::from_flags(true, true),
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        assert_eq!(outcome.reused_files, 0);

        let hash = outcome
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");
        let stored = fs::read(store.object_path(&hash)?)?;
        assert_eq!(
            stored[crate::object::MAGIC.len()],
            crate::object::CODEC_ZSTD
        );
        Ok(())
    }

    // ============================================================================
    // --compress だけなら未変更は再利用しつつ、新規は圧縮できることを確認する。
    // ============================================================================
    #[test]
    fn compress_alone_still_reuses_unchanged() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        fs::write(target.join("note.txt"), "hello")?;

        let store = Store::init(&target, None)?;
        create(&store, None)?;
        let second = create_with_pace(
            &store,
            Some("compress".into()),
            SnapshotOptions::from_flags(false, true),
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        assert!(second.reused_files >= 1);
        Ok(())
    }

    // ============================================================================
    // size/mtime が同じまま中身だけ変わった場合、--rehash は新しいハッシュを取る。
    // ============================================================================
    #[test]
    fn rehash_detects_same_size_content_change() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir_all(&target)?;
        let file = target.join("note.txt");
        fs::write(&file, "hello")?;

        let store = Store::init(&target, None)?;
        let first = create(&store, Some("first".into()))?;
        let first_hash = first
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");

        let original_mtime = filetime::FileTime::from_last_modification_time(&fs::metadata(&file)?);
        fs::write(&file, "world")?;
        filetime::set_file_mtime(&file, original_mtime)?;

        let slipped = create(&store, Some("slipped".into()))?;
        let slipped_hash = slipped
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");
        assert!(slipped.reused_files >= 1);
        assert_eq!(slipped_hash, first_hash);

        let rehashed = create_with_pace(
            &store,
            Some("rehash".into()),
            SnapshotOptions::from_flags(true, false),
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        let rehash_hash = rehashed
            .manifest
            .entries
            .iter()
            .find(|entry| entry.path.ends_with("note.txt"))
            .and_then(|entry| entry.object.clone())
            .expect("note.txt object");
        assert_eq!(rehashed.reused_files, 0);
        assert_ne!(rehash_hash, first_hash);
        Ok(())
    }
}
