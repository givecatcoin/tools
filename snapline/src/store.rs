//! ============================================================================
//! 履歴ストアのディレクトリ構造と、設定・マニフェストの読み書き。
//!
//! 既定では対象ツリー直下に `.snapline/` を置く。
//! `--store` で別場所を指定した場合は、ツリー直下の `.snapline` を
//! 外部ストアへのポインタファイルとして残す。
//!
//! .snapline/   ... ストア本体（またはポインタファイル）
//!   config.json
//!   objects/
//!   snapshots/
//!   summaries/
//!   tmp/
//! ============================================================================

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    model::{FORMAT_VERSION, SnapshotManifest, SnapshotSummary, StoreConfig},
    settings::UserSettings,
};

const CONFIG_FILE: &str = "config.json";
pub const STORE_DIR: &str = ".snapline";
pub const SUMMARIES_DIR: &str = "summaries";
const POINTER_PREFIX: &str = "SNAPLINE_STORE=";
const MIN_SHORT_ID_LENGTH: usize = 4;

// ============================================================================
// 履歴ストア一式への入口。設定・オブジェクト・マニフェストを束ねる。
// ============================================================================
pub struct Store {
    /// canonicalize 済みのストアルート。
    pub root: PathBuf,
    pub config: StoreConfig,
}

// ============================================================================
// 書き込み排他用ロック。
// Drop でロックファイルを消す。
// プロセス異常終了時は手動削除が必要になる場合がある。
// ============================================================================
pub struct StoreLock {
    path: PathBuf,
}

impl Store {
    // ============================================================================
    // 新規ストアを作る。
    // store_path が None なら対象ツリー直下の `.snapline` を使う。
    // 指定がある場合はその場所をストア本体にし、ツリー直下へポインタを書く。
    // ============================================================================
    pub fn init(target_path: &Path, store_path: Option<&Path>) -> Result<Self> {
        let target = target_path
            .canonicalize()
            .with_context(|| format!("target does not exist: {}", target_path.display()))?;
        let resolved_store = resolve_store_location(&target, store_path)?;

        prepare_empty_store_dir(&resolved_store)?;
        let root = resolved_store
            .canonicalize()
            .with_context(|| format!("failed to resolve store: {}", resolved_store.display()))?;

        fs::create_dir_all(root.join("objects"))?;
        fs::create_dir_all(root.join("snapshots"))?;
        fs::create_dir_all(root.join(SUMMARIES_DIR))?;
        fs::create_dir_all(root.join("tmp"))?;

        let config = StoreConfig {
            format_version: FORMAT_VERSION,
            target: target.clone(),
            settings: UserSettings::defaults(),
        };
        write_json_atomic(&root.join(CONFIG_FILE), &config, &root.join("tmp"))?;

        // ツリー側からストアを辿れるように、直下の `.snapline` を整える。
        write_tree_marker(&target, &root)?;

        Ok(Self { root, config })
    }

    // ============================================================================
    // 既存ストアを開く。
    // store_path が指定されていればそれを優先し、無ければツリー直下の
    // `.snapline`（ディレクトリ本体またはポインタファイル）を辿る。
    // ============================================================================
    pub fn open(target_path: &Path, store_path: Option<&Path>) -> Result<Self> {
        let target = target_path
            .canonicalize()
            .with_context(|| format!("target does not exist: {}", target_path.display()))?;
        let root = match store_path {
            Some(path) => resolve_store_location(&target, Some(path))?
                .canonicalize()
                .with_context(|| format!("store does not exist: {}", path.display()))?,
            None => locate_store_from_tree(&target)?,
        };

        let mut config: StoreConfig = read_json(&root.join(CONFIG_FILE))?;
        if config.format_version != FORMAT_VERSION {
            bail!(
                "unsupported store format {}, expected {}",
                config.format_version,
                FORMAT_VERSION
            );
        }
        // 設定ファイル内の古い絶対パスへ誘導されないよう、呼び出し側のツリーで上書きする。
        config.target = target;
        Ok(Self { root, config })
    }

    // ============================================================================
    // 内容ハッシュからオブジェクトパスを組み立てる。
    // 先頭 2 文字でサブディレクトリを切る（fan-out）。
    // ============================================================================
    pub fn object_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid object hash: {hash}");
        }
        Ok(self.root.join("objects").join(&hash[..2]).join(&hash[2..]))
    }

    // ============================================================================
    // スナップショット ID からマニフェストパスを組み立てる。
    // ============================================================================
    pub fn manifest_path(&self, id: &str) -> Result<PathBuf> {
        validate_snapshot_id(id)?;
        Ok(self.root.join("snapshots").join(format!("{id}.json")))
    }

    // ============================================================================
    // 完全 ID または一意な短縮 ID でマニフェストを読む。
    // 短縮 ID は完全 ID の先頭、または末尾 UUID 部分の先頭として照合する。
    // ============================================================================
    pub fn read_manifest(&self, id: &str) -> Result<SnapshotManifest> {
        let resolved = self.resolve_snapshot_id(id)?;
        read_json(&self.manifest_path(&resolved)?)
    }

    // ============================================================================
    // 入力された ID をストア内の完全なスナップショット ID へ解決する。
    // 完全一致を優先し、短縮 ID が複数件に一致する場合は誤復元を防ぐため拒否する。
    // ============================================================================
    pub fn resolve_snapshot_id(&self, id: &str) -> Result<String> {
        validate_snapshot_id(id)?;
        if self.manifest_path(id)?.is_file() {
            return Ok(id.to_owned());
        }
        if id.len() < MIN_SHORT_ID_LENGTH {
            bail!("short snapshot id must be at least {MIN_SHORT_ID_LENGTH} characters: {id}");
        }

        let mut matches = Vec::new();
        for item in fs::read_dir(self.root.join("snapshots"))? {
            let path = item?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(candidate) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            let uuid_part = candidate.rsplit_once('-').map(|(_, suffix)| suffix);
            if candidate.starts_with(id) || uuid_part.is_some_and(|suffix| suffix.starts_with(id)) {
                matches.push(candidate.to_owned());
            }
        }

        match matches.as_slice() {
            [resolved] => Ok(resolved.clone()),
            [] => bail!("snapshot does not exist: {id}"),
            _ => bail!("snapshot id is ambiguous: {id} ({} matches)", matches.len()),
        }
    }

    // ============================================================================
    // マニフェストを原子的に書き込み、log 用要約も同時に残す。
    // ============================================================================
    pub fn write_manifest(&self, manifest: &SnapshotManifest) -> Result<()> {
        write_json_atomic(
            &self.manifest_path(&manifest.id)?,
            manifest,
            &self.root.join("tmp"),
        )?;
        self.write_summary(&SnapshotSummary::from_manifest(manifest))
    }

    // ============================================================================
    // スナップショット要約の保存先パス。
    // ============================================================================
    pub fn summary_path(&self, id: &str) -> Result<PathBuf> {
        validate_snapshot_id(id)?;
        Ok(self
            .root
            .join(SUMMARIES_DIR)
            .join(format!("{id}.json")))
    }

    // ============================================================================
    // log 用要約を原子的に書き込む。
    // ============================================================================
    pub fn write_summary(&self, summary: &SnapshotSummary) -> Result<()> {
        fs::create_dir_all(self.root.join(SUMMARIES_DIR))?;
        write_json_atomic(
            &self.summary_path(&summary.id)?,
            summary,
            &self.root.join("tmp"),
        )
    }

    // ============================================================================
    // 要約を読む。欠ける・壊れている場合は Err。
    // ============================================================================
    pub fn read_summary(&self, id: &str) -> Result<SnapshotSummary> {
        read_json(&self.summary_path(id)?)
    }

    // ============================================================================
    // snapshots/ 内のマニフェスト JSON パスを列挙する（名前順）。
    // ============================================================================
    pub fn snapshot_manifest_paths(&self) -> Result<Vec<PathBuf>> {
        let mut paths = Vec::new();
        let dir = self.root.join("snapshots");
        if !dir.exists() {
            return Ok(paths);
        }
        for item in fs::read_dir(dir)? {
            let path = item?.path();
            if path.extension().and_then(|value| value.to_str()) == Some("json") {
                paths.push(path);
            }
        }
        paths.sort();
        Ok(paths)
    }

    // ============================================================================
    // スナップショット書き込み用の排他ロックを取る。
    // ============================================================================
    pub fn lock(&self) -> Result<StoreLock> {
        let path = self.root.join("write.lock");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| {
                format!(
                    "store is locked; if no process is running, remove {}",
                    path.display()
                )
            })?;
        writeln!(file, "pid={}", std::process::id())?;
        file.sync_all()?;
        Ok(StoreLock { path })
    }
}

// ============================================================================
// 開始ディレクトリから親方向へ `.snapline` を探し、対象ツリーを返す。
// 最も近い `.snapline` を採用し、ファイルかディレクトリかの検証は open に任せる。
// ============================================================================
pub fn discover_tree_root(start: &Path) -> Result<PathBuf> {
    let start = start
        .canonicalize()
        .with_context(|| format!("path does not exist: {}", start.display()))?;
    let mut current = start.as_path();

    loop {
        if current.join(STORE_DIR).exists() {
            return Ok(current.to_path_buf());
        }
        current = current.parent().with_context(|| {
            format!(
                "no .snapline found in {} or any parent; run `snapline init` first",
                start.display()
            )
        })?;
    }
}

// ============================================================================
// スナップショット ID にパス区切りなどが含まれていないことを検査する。
// ============================================================================
fn validate_snapshot_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        bail!("invalid snapshot id: {id}");
    }
    Ok(())
}

impl Drop for StoreLock {
    // ============================================================================
    // スコープ終了時にロックファイルを削除する。
    // ============================================================================
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

// ============================================================================
// ストア本体の配置場所を決定する。
// 未指定なら `<tree>/.snapline`。
// 指定パスの末尾が `.snapline` ならそのまま、そうでなければ直下に `.snapline` を足す。
// ============================================================================
fn resolve_store_location(target: &Path, store_path: Option<&Path>) -> Result<PathBuf> {
    let Some(store_path) = store_path else {
        return Ok(target.join(STORE_DIR));
    };

    let name = store_path.file_name().and_then(|value| value.to_str());
    let resolved = if name == Some(STORE_DIR) {
        store_path.to_path_buf()
    } else {
        store_path.join(STORE_DIR)
    };

    // 対象ツリーの内側に置く場合は、誤って親子逆転しないよう注意喚起のみ。
    // 内側配置でも走査時にストア本体は除外する。
    let _ = target;
    Ok(resolved)
}

// ============================================================================
// 空のストアディレクトリを用意する。既存の中身がある場合は拒否する。
// ============================================================================
fn prepare_empty_store_dir(store_path: &Path) -> Result<()> {
    if store_path.exists() {
        let metadata = fs::symlink_metadata(store_path)?;
        if !metadata.is_dir() {
            bail!(
                "store path exists and is not a directory: {}",
                store_path.display()
            );
        }
        if store_path.read_dir()?.next().is_some() {
            bail!("store directory is not empty: {}", store_path.display());
        }
    } else {
        fs::create_dir_all(store_path)
            .with_context(|| format!("failed to create store: {}", store_path.display()))?;
    }
    Ok(())
}

// ============================================================================
// ツリー直下の `.snapline` を、ストア本体または外部ポインタとして整える。
// ============================================================================
fn write_tree_marker(target: &Path, store_root: &Path) -> Result<()> {
    let marker = target.join(STORE_DIR);
    let default_store = target.join(STORE_DIR);

    if paths_equal(store_root, &default_store)? {
        // 直下配置: marker 自体がストア本体なので追加処理は不要。
        return Ok(());
    }

    if marker.exists() {
        let metadata = fs::symlink_metadata(&marker)?;
        if metadata.is_dir() {
            if marker.read_dir()?.next().is_some() {
                bail!(
                    "cannot write store pointer; {} already exists as a non-empty directory",
                    marker.display()
                );
            }
            fs::remove_dir(&marker)?;
        } else {
            fs::remove_file(&marker)?;
        }
    }

    let content = format!("{POINTER_PREFIX}{}\n", store_root.display());
    fs::write(&marker, content)
        .with_context(|| format!("failed to write store pointer: {}", marker.display()))?;
    Ok(())
}

// ============================================================================
// ツリー直下の `.snapline` からストア本体を解決する。
// ============================================================================
fn locate_store_from_tree(target: &Path) -> Result<PathBuf> {
    let marker = target.join(STORE_DIR);
    if !marker.exists() {
        bail!("store does not exist: {}", marker.display());
    }

    let metadata = fs::symlink_metadata(&marker)
        .with_context(|| format!("failed to inspect {}", marker.display()))?;
    if metadata.is_dir() {
        return marker
            .canonicalize()
            .with_context(|| format!("store does not exist: {}", marker.display()));
    }
    if !metadata.is_file() {
        bail!("unsupported .snapline marker type: {}", marker.display());
    }

    let text = fs::read_to_string(&marker)
        .with_context(|| format!("failed to read store pointer: {}", marker.display()))?;
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .context("store pointer file is empty")?;
    let path_text = line.strip_prefix(POINTER_PREFIX).unwrap_or(line).trim();
    PathBuf::from(path_text).canonicalize().with_context(|| {
        format!(
            "store pointed by {} does not exist: {path_text}",
            marker.display()
        )
    })
}

// ============================================================================
// canonicalize してパス同一性を比べる。
// ============================================================================
fn paths_equal(left: &Path, right: &Path) -> Result<bool> {
    let left = if left.exists() {
        left.canonicalize()?
    } else {
        left.to_path_buf()
    };
    let right = if right.exists() {
        right.canonicalize()?
    } else {
        right.to_path_buf()
    };
    Ok(left == right)
}

// ============================================================================
// JSON を読み込む共通処理。
// ============================================================================
pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let file = File::open(path)
        .with_context(|| format!("failed to open JSON file: {}", path.display()))?;
    serde_json::from_reader(file)
        .with_context(|| format!("failed to read JSON file: {}", path.display()))
}

// ============================================================================
// 一時ファイルへ書いてからリネームする原子的 JSON 書き込み。
// ============================================================================
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T, temp_dir: &Path) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(temp_dir)?;
    serde_json::to_writer_pretty(&mut temp, value)?;
    temp.write_all(b"\n")?;
    temp.as_file().sync_all()?;
    temp.persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to persist JSON file: {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::{STORE_DIR, Store, discover_tree_root};

    // ============================================================================
    // 既定では対象ツリー直下に `.snapline` が作られることを確認する。
    // ============================================================================
    #[test]
    fn initializes_store_directly_under_target_tree() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir(&target)?;

        let store = Store::init(&target, None)?;

        assert_eq!(store.root, target.join(STORE_DIR).canonicalize()?);
        assert!(target.join(STORE_DIR).join("config.json").is_file());
        assert!(target.join(STORE_DIR).join("summaries").is_dir());
        Ok(())
    }

    // ============================================================================
    // `--store` で直下以外へ置き、ポインタ経由で開けることを確認する。
    // ============================================================================
    #[test]
    fn initializes_external_store_with_pointer() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        let external = root.path().join("external-parent");
        fs::create_dir(&target)?;
        fs::create_dir(&external)?;

        let store = Store::init(&target, Some(&external))?;
        assert_eq!(store.root, external.join(STORE_DIR).canonicalize()?);
        assert!(target.join(STORE_DIR).is_file());

        let opened = Store::open(&target, None)?;
        assert_eq!(opened.root, store.root);
        assert_eq!(opened.config.target, target.canonicalize()?);
        Ok(())
    }

    // ============================================================================
    // ツリー移動後も直下ストアを継続利用できることを確認する。
    // ============================================================================
    #[test]
    fn opens_store_after_target_tree_is_moved() -> Result<()> {
        let root = tempfile::tempdir()?;
        let original = root.path().join("original");
        let moved = root.path().join("moved");
        fs::create_dir(&original)?;
        Store::init(&original, None)?;
        fs::rename(&original, &moved)?;

        let store = Store::open(&moved, None)?;
        assert_eq!(store.config.target, moved.canonicalize()?);
        Ok(())
    }

    // ============================================================================
    // サブディレクトリから親の `.snapline` を発見できることを確認する。
    // ============================================================================
    #[test]
    fn discovers_tree_root_from_nested_directory() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        let nested = target.join("project/src");
        fs::create_dir_all(&nested)?;
        Store::init(&target, None)?;

        assert_eq!(discover_tree_root(&nested)?, target.canonicalize()?);
        Ok(())
    }

    // ============================================================================
    // UUID 部分の短縮 ID が一意な完全 ID に解決されることを確認する。
    // ============================================================================
    #[test]
    fn resolves_unique_short_snapshot_id() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir(&target)?;
        let store = Store::init(&target, None)?;
        let full_id = "20260720T010000000Z-abcdef0123456789";
        fs::write(store.manifest_path(full_id)?, "{}")?;

        assert_eq!(store.resolve_snapshot_id("abcdef01")?, full_id);
        Ok(())
    }

    // ============================================================================
    // 複数候補に一致する短縮 ID は拒否されることを確認する。
    // ============================================================================
    #[test]
    fn rejects_ambiguous_short_snapshot_id() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("workspace");
        fs::create_dir(&target)?;
        let store = Store::init(&target, None)?;
        fs::write(store.manifest_path("20260720T010000000Z-abcd1111")?, "{}")?;
        fs::write(store.manifest_path("20260720T010000001Z-abcd2222")?, "{}")?;

        let error = store.resolve_snapshot_id("abcd").unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
        Ok(())
    }
}
