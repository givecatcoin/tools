//! ============================================================================
//! 内容アドレス化されたオブジェクトの取り込みと読み出し。
//!
//! ハッシュは「元のファイル内容」の SHA-256。
//! 圧縮して保存してもハッシュは変えない。
//! これにより、圧縮有無が違ってもスナップショット間で同じ内容を共有できる。
//!
//! オンディスク形式（過去ストア互換・維持）:
//!   SNAPOBJ1  (8 bytes magic)
//!   codec     (1 byte: 0=raw, 1=zstd)
//!   payload   (raw bytes or zstd frame)
//!
//! magic が無い旧オブジェクトは、生バイト列として読む（後方互換）。
//! ============================================================================

use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::{pace::IoPace, progress::Progress, store::Store};

/// 現行オブジェクト形式の識別子。
pub(crate) const MAGIC: &[u8; 8] = b"SNAPOBJ1";

/// 圧縮せずそのまま格納する。
pub(crate) const CODEC_RAW: u8 = 0;

/// Zstandard で圧縮して格納する。
pub(crate) const CODEC_ZSTD: u8 = 1;

/// `snap --compress` 用。対話的取得向けで速度と比率のバランスが良い。
const COMPRESSION_LEVEL_FULL: i32 = 3;

/// `care` 一括圧縮用。頻度が低いのでやや比率寄り。形式にはレベルを残さない。
const COMPRESSION_LEVEL_CARE: i32 = 5;

const BUFFER_SIZE: usize = 1024 * 1024;

// ============================================================================
// 取り込み時の保存方針。ハッシュ計算はどちらの場合も元内容に対して行う。
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestMode {
    /// 常に raw（普段の snap）。
    Raw,
    /// zstd を試し、効かなければ raw（`snap --compress`）。
    Compress,
}

// ============================================================================
// 一括圧縮の結果。
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactOutcome {
    pub compressed: usize,
    pub unchanged: usize,
}

// ============================================================================
// ファイルをストアへ取り込み、内容ハッシュを返す（通常優先度・圧縮あり）。
// ============================================================================
#[allow(dead_code)]
pub fn ingest(store: &Store, path: &Path, before: &Metadata) -> Result<String> {
    ingest_with_pace(
        store,
        path,
        before,
        IngestMode::Compress,
        &mut crate::pace::IdlePace,
    )
}

// ============================================================================
// ペース制御付きの取り込み。
// ============================================================================
pub fn ingest_with_pace(
    store: &Store,
    path: &Path,
    before: &Metadata,
    mode: IngestMode,
    pace: &mut dyn IoPace,
) -> Result<String> {
    match mode {
        IngestMode::Raw => ingest_raw(store, path, before, pace),
        IngestMode::Compress => ingest_compress(store, path, before, COMPRESSION_LEVEL_FULL, pace),
    }
}

// ============================================================================
// 無圧縮で取り込む（1 パス）。
// ============================================================================
fn ingest_raw(
    store: &Store,
    path: &Path,
    before: &Metadata,
    pace: &mut dyn IoPace,
) -> Result<String> {
    let mut source =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(store.root.join("tmp"))?;
    temp.write_all(MAGIC)?;
    temp.write_all(&[CODEC_RAW])?;

    let mut hasher = Sha256::new();
    copy_and_hash(&mut source, &mut temp, &mut hasher, pace)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let hash = format!("{:x}", hasher.finalize());

    ensure_unchanged(path, before)?;
    persist_new_object(store, &hash, temp)?;
    Ok(hash)
}

// ============================================================================
// zstd を試し、効かなければ raw に作り直す。
// ============================================================================
fn ingest_compress(
    store: &Store,
    path: &Path,
    before: &Metadata,
    level: i32,
    pace: &mut dyn IoPace,
) -> Result<String> {
    let mut source =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(store.root.join("tmp"))?;

    temp.write_all(MAGIC)?;
    temp.write_all(&[CODEC_ZSTD])?;

    let mut hasher = Sha256::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut temp, level)?;
        copy_and_hash(&mut source, &mut encoder, &mut hasher, pace)
            .with_context(|| format!("failed to read {}", path.display()))?;
        encoder.finish()?;
    }

    let hash = format!("{:x}", hasher.finalize());
    let compressed_payload_size = temp.as_file().metadata()?.len() - (MAGIC.len() as u64 + 1);

    if compressed_payload_size >= before.len() {
        temp.as_file_mut().set_len(0)?;
        temp.as_file_mut().seek(SeekFrom::Start(0))?;
        temp.write_all(MAGIC)?;
        temp.write_all(&[CODEC_RAW])?;

        let mut second_source = File::open(path)?;
        let mut second_hasher = Sha256::new();
        copy_and_hash(&mut second_source, &mut temp, &mut second_hasher, pace)?;
        let second_hash = format!("{:x}", second_hasher.finalize());
        if second_hash != hash {
            bail!("file changed while being captured: {}", path.display());
        }
    }

    ensure_unchanged(path, before)?;
    let wrote_zstd = {
        let file = temp.as_file_mut();
        file.seek(SeekFrom::Start(MAGIC.len() as u64))?;
        let mut codec = [0_u8; 1];
        file.read_exact(&mut codec)?;
        file.seek(SeekFrom::Start(0))?;
        codec[0] == CODEC_ZSTD
    };
    persist_or_upgrade(store, &hash, temp, wrote_zstd)?;
    Ok(hash)
}

// ============================================================================
// マニフェストから参照されるオブジェクトのうち、圧縮が効くものを zstd へ差し替える。
// ハッシュは変えない。既に zstd のもの・効かないものは触らない。
// 旧形式（ヘッダ無し）も、圧縮が効けば SNAPOBJ1+zstd へ正規化する。
// ============================================================================
pub fn compact_referenced_with_pace(
    store: &Store,
    objects: &[(String, u64)],
    pace: &mut dyn IoPace,
    progress: &mut Progress,
) -> Result<CompactOutcome> {
    let total = objects.len();
    progress.begin("Compacting objects");
    let mut compressed = 0_usize;
    let mut unchanged = 0_usize;

    for (index, (hash, expected_size)) in objects.iter().enumerate() {
        pace.before_entry()?;
        match try_compact_one(store, hash, *expected_size, pace)? {
            CompactOne::Compressed => compressed += 1,
            CompactOne::Unchanged => unchanged += 1,
        }
        progress.ratio(index + 1, total);
    }

    if total == 0 {
        progress.done("0 objects, done.");
    } else {
        progress.done(&format!(
            "{compressed} compressed, {unchanged} unchanged, done."
        ));
    }

    Ok(CompactOutcome {
        compressed,
        unchanged,
    })
}

#[derive(Debug, Clone, Copy)]
enum CompactOne {
    Compressed,
    Unchanged,
}

// ============================================================================
// 1 オブジェクトを必要なら圧縮して置き換える。
// ============================================================================
fn try_compact_one(
    store: &Store,
    hash: &str,
    expected_size: u64,
    pace: &mut dyn IoPace,
) -> Result<CompactOne> {
    let path = store.object_path(hash)?;
    if matches!(inspect_codec(&path)?, ObjectCodec::Zstd) {
        return Ok(CompactOne::Unchanged);
    }

    let mut content = Vec::new();
    let mut hasher = Sha256::new();
    {
        let mut source = open_file_reader(&path, hash)?;
        let mut buffer = vec![0_u8; BUFFER_SIZE];
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            content.extend_from_slice(&buffer[..read]);
            pace.after_chunk(read)?;
        }
    }

    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != hash || content.len() as u64 != expected_size {
        bail!("object failed integrity check before compact: {hash}");
    }

    let mut encoded = Vec::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut encoded, COMPRESSION_LEVEL_CARE)?;
        encoder.write_all(&content)?;
        encoder.finish()?;
    }

    if encoded.len() as u64 >= expected_size {
        return Ok(CompactOne::Unchanged);
    }

    let mut temp = tempfile::NamedTempFile::new_in(store.root.join("tmp"))?;
    temp.write_all(MAGIC)?;
    temp.write_all(&[CODEC_ZSTD])?;
    temp.write_all(&encoded)?;
    temp.as_file().sync_all()?;
    replace_object(store, hash, temp)?;
    Ok(CompactOne::Compressed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectCodec {
    RawOrLegacy,
    Zstd,
}

// ============================================================================
// オブジェクトの codec をヘッダだけ見て判別する。
// ============================================================================
fn inspect_codec(path: &Path) -> Result<ObjectCodec> {
    let mut file = File::open(path)?;
    let mut magic = [0_u8; MAGIC.len()];
    match file.read_exact(&mut magic) {
        Ok(()) if &magic == MAGIC => {
            let mut codec = [0_u8; 1];
            file.read_exact(&mut codec)?;
            match codec[0] {
                CODEC_ZSTD => Ok(ObjectCodec::Zstd),
                CODEC_RAW => Ok(ObjectCodec::RawOrLegacy),
                value => bail!(
                    "object uses unsupported codec {value}: {}",
                    path.display()
                ),
            }
        }
        Ok(()) | Err(_) => Ok(ObjectCodec::RawOrLegacy),
    }
}

// ============================================================================
// オブジェクトの簡易整合チェック。
// ============================================================================
pub fn looks_consistent(store: &Store, hash: &str) -> Result<bool> {
    let path = store.object_path(hash)?;
    if !path.is_file() {
        return Ok(false);
    }

    let mut file = File::open(&path).with_context(|| format!("failed to open object {hash}"))?;
    let mut magic = [0_u8; MAGIC.len()];
    match file.read_exact(&mut magic) {
        Ok(()) if &magic == MAGIC => {
            let mut codec = [0_u8; 1];
            if file.read_exact(&mut codec).is_err() {
                return Ok(false);
            }
            Ok(matches!(codec[0], CODEC_RAW | CODEC_ZSTD))
        }
        Ok(()) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(true),
        Err(error) => Err(error).with_context(|| format!("failed to inspect object {hash}")),
    }
}

// ============================================================================
// オブジェクトを展開しつつハッシュ検証する（通常優先度）。
// ============================================================================
#[allow(dead_code)]
pub fn copy_verified(
    store: &Store,
    expected_hash: &str,
    expected_size: u64,
    mut output: impl Write,
) -> Result<()> {
    copy_verified_with_pace(
        store,
        expected_hash,
        expected_size,
        &mut output,
        &mut crate::pace::IdlePace,
    )
}

// ============================================================================
// ペース制御付きの検証付きコピー。
// ============================================================================
pub fn copy_verified_with_pace(
    store: &Store,
    expected_hash: &str,
    expected_size: u64,
    mut output: impl Write,
    pace: &mut dyn IoPace,
) -> Result<()> {
    let mut source = open(store, expected_hash)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    let mut size = 0_u64;

    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        output.write_all(&buffer[..read])?;
        size += read as u64;
        pace.after_chunk(read)?;
    }

    let actual_hash = format!("{:x}", hasher.finalize());
    if actual_hash != expected_hash || size != expected_size {
        bail!("object failed integrity check: {expected_hash}");
    }

    Ok(())
}

// ============================================================================
// 保存形式を判別して、元内容を読める Read を返す。
// ============================================================================
fn open(store: &Store, hash: &str) -> Result<Box<dyn Read>> {
    open_file_reader(&store.object_path(hash)?, hash)
}

// ============================================================================
// ファイルパスからオブジェクト Read を開く。
// ============================================================================
fn open_file_reader(path: &Path, hash: &str) -> Result<Box<dyn Read>> {
    let mut file = File::open(path).with_context(|| format!("object is missing: {hash}"))?;
    let mut magic = [0_u8; MAGIC.len()];

    match file.read_exact(&mut magic) {
        Ok(()) if &magic == MAGIC => {}
        Ok(()) => {
            file.seek(SeekFrom::Start(0))?;
            return Ok(Box::new(file));
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            file.seek(SeekFrom::Start(0))?;
            return Ok(Box::new(file));
        }
        Err(error) => return Err(error.into()),
    }

    let mut codec = [0_u8; 1];
    file.read_exact(&mut codec)?;
    match codec[0] {
        CODEC_RAW => Ok(Box::new(file)),
        CODEC_ZSTD => Ok(Box::new(zstd::stream::read::Decoder::new(file)?)),
        value => bail!("object uses unsupported codec {value}: {hash}"),
    }
}

// ============================================================================
// 新規オブジェクトのみ persist する（既存があれば中身は書かない）。
// ============================================================================
fn persist_new_object(
    store: &Store,
    hash: &str,
    temp: tempfile::NamedTempFile,
) -> Result<()> {
    temp.as_file().sync_all()?;
    let destination = store.object_path(hash)?;
    if destination.exists() {
        return Ok(());
    }
    let parent = destination
        .parent()
        .context("object path has no parent directory")?;
    fs::create_dir_all(parent)?;
    match temp.persist_noclobber(&destination) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error)
            .with_context(|| format!("failed to persist object {}", destination.display())),
    }
}

// ============================================================================
// 新規なら保存。既存が raw/旧形式で今回 zstd なら置き換える（`--compress` 用）。
// ============================================================================
fn persist_or_upgrade(
    store: &Store,
    hash: &str,
    temp: tempfile::NamedTempFile,
    wrote_zstd: bool,
) -> Result<()> {
    let destination = store.object_path(hash)?;
    if !destination.exists() {
        return persist_new_object(store, hash, temp);
    }
    if wrote_zstd && matches!(inspect_codec(&destination)?, ObjectCodec::RawOrLegacy) {
        return replace_object(store, hash, temp);
    }
    Ok(())
}

// ============================================================================
// 既存オブジェクトを新しい内容で原子的に置き換える（care 用）。
// ============================================================================
fn replace_object(store: &Store, hash: &str, temp: tempfile::NamedTempFile) -> Result<()> {
    let destination = store.object_path(hash)?;
    let parent = destination
        .parent()
        .context("object path has no parent directory")?;
    fs::create_dir_all(parent)?;

    let staging = staging_path(&destination);
    if staging.exists() {
        fs::remove_file(&staging)?;
    }
    temp.as_file().sync_all()?;
    temp.persist(&staging)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to stage object {}", staging.display()))?;

    let backup = backup_path(&destination);
    if backup.exists() {
        fs::remove_file(&backup)?;
    }
    if destination.exists() {
        fs::rename(&destination, &backup)?;
    }
    match fs::rename(&staging, &destination) {
        Ok(()) => {
            let _ = fs::remove_file(&backup);
            Ok(())
        }
        Err(error) => {
            if backup.exists() && !destination.exists() {
                let _ = fs::rename(&backup, &destination);
            }
            let _ = fs::remove_file(&staging);
            Err(error).with_context(|| {
                format!("failed to replace object {}", destination.display())
            })
        }
    }
}

fn staging_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(".new");
    destination.with_file_name(name)
}

fn backup_path(destination: &Path) -> PathBuf {
    let mut name = destination
        .file_name()
        .map(|value| value.to_os_string())
        .unwrap_or_default();
    name.push(".bak");
    destination.with_file_name(name)
}

// ============================================================================
// 取り込み前後で size/mtime が変わっていないことを確認する。
// ============================================================================
fn ensure_unchanged(path: &Path, before: &Metadata) -> Result<()> {
    let after = fs::metadata(path)?;
    if before.len() != after.len() || modified_nanos(before) != modified_nanos(&after) {
        bail!("file changed while being captured: {}", path.display());
    }
    Ok(())
}

// ============================================================================
// 読みながらハッシュ更新と書き込みを同時に行う。
// ============================================================================
fn copy_and_hash(
    source: &mut impl Read,
    destination: &mut impl Write,
    hasher: &mut Sha256,
    pace: &mut dyn IoPace,
) -> Result<()> {
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let read = source.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        destination.write_all(&buffer[..read])?;
        pace.after_chunk(read)?;
    }
    Ok(())
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
    use sha2::{Digest, Sha256};

    use super::{
        CODEC_RAW, CODEC_ZSTD, IngestMode, MAGIC, compact_referenced_with_pace, copy_verified,
        ingest, ingest_with_pace, looks_consistent,
    };
    use crate::{pace::IdlePace, progress::Progress, store::Store};

    // ============================================================================
    // 繰り返しデータが圧縮され、復元できることを確認する。
    // ============================================================================
    #[test]
    fn compresses_repetitive_content_and_restores_it() -> Result<()> {
        let fixture = Fixture::new(&vec![b'a'; 128 * 1024])?;
        let hash = ingest(&fixture.store, &fixture.file, &fs::metadata(&fixture.file)?)?;
        let stored = fs::read(fixture.store.object_path(&hash)?)?;

        assert_eq!(&stored[..MAGIC.len()], MAGIC);
        assert_eq!(stored[MAGIC.len()], CODEC_ZSTD);
        assert!(stored.len() < 1024);

        let mut restored = Vec::new();
        copy_verified(&fixture.store, &hash, 128 * 1024, &mut restored)?;
        assert_eq!(restored, vec![b'a'; 128 * 1024]);
        Ok(())
    }

    // ============================================================================
    // 非圧縮取り込みが raw で保存されることを確認する。
    // ============================================================================
    #[test]
    fn stores_raw_without_compression() -> Result<()> {
        let fixture = Fixture::new(&vec![b'a'; 128 * 1024])?;
        let hash = ingest_with_pace(
            &fixture.store,
            &fixture.file,
            &fs::metadata(&fixture.file)?,
            IngestMode::Raw,
            &mut IdlePace,
        )?;
        let stored = fs::read(fixture.store.object_path(&hash)?)?;
        assert_eq!(&stored[..MAGIC.len()], MAGIC);
        assert_eq!(stored[MAGIC.len()], CODEC_RAW);
        assert!(stored.len() > 128 * 1024);
        Ok(())
    }

    // ============================================================================
    // care 相当の一括圧縮で raw が zstd になることを確認する。
    // ============================================================================
    #[test]
    fn compact_converts_raw_repetitive_object_to_zstd() -> Result<()> {
        let fixture = Fixture::new(&vec![b'a'; 128 * 1024])?;
        let hash = ingest_with_pace(
            &fixture.store,
            &fixture.file,
            &fs::metadata(&fixture.file)?,
            IngestMode::Raw,
            &mut IdlePace,
        )?;
        let before = fs::metadata(fixture.store.object_path(&hash)?)?.len();
        let outcome = compact_referenced_with_pace(
            &fixture.store,
            &[(hash.clone(), 128 * 1024)],
            &mut IdlePace,
            &mut Progress::quiet(),
        )?;
        let after = fs::metadata(fixture.store.object_path(&hash)?)?.len();
        let stored = fs::read(fixture.store.object_path(&hash)?)?;

        assert_eq!(outcome.compressed, 1);
        assert!(after < before);
        assert_eq!(stored[MAGIC.len()], CODEC_ZSTD);

        let mut restored = Vec::new();
        copy_verified(&fixture.store, &hash, 128 * 1024, &mut restored)?;
        assert_eq!(restored, vec![b'a'; 128 * 1024]);
        Ok(())
    }

    // ============================================================================
    // 圧縮効果のないデータは raw で保持されることを確認する。
    // ============================================================================
    #[test]
    fn keeps_incompressible_content_raw() -> Result<()> {
        let mut value = 0x1234_5678_9abc_def0_u64;
        let content: Vec<u8> = (0..64 * 1024)
            .map(|_| {
                value ^= value << 13;
                value ^= value >> 7;
                value ^= value << 17;
                value as u8
            })
            .collect();
        let fixture = Fixture::new(&content)?;
        let hash = ingest(&fixture.store, &fixture.file, &fs::metadata(&fixture.file)?)?;
        let stored = fs::read(fixture.store.object_path(&hash)?)?;

        assert_eq!(&stored[..MAGIC.len()], MAGIC);
        assert_eq!(stored[MAGIC.len()], CODEC_RAW);

        let mut restored = Vec::new();
        copy_verified(&fixture.store, &hash, content.len() as u64, &mut restored)?;
        assert_eq!(restored, content);
        Ok(())
    }

    // ============================================================================
    // 簡易整合は存在する正規オブジェクトで true になることを確認する。
    // ============================================================================
    #[test]
    fn looks_consistent_accepts_stored_object() -> Result<()> {
        let fixture = Fixture::new(b"consistent")?;
        let hash = ingest(&fixture.store, &fixture.file, &fs::metadata(&fixture.file)?)?;
        assert!(looks_consistent(&fixture.store, &hash)?);
        assert!(!looks_consistent(
            &fixture.store,
            "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
        )?);
        Ok(())
    }

    // ============================================================================
    // 圧縮導入前のヘッダ無しオブジェクトも読めることを確認する。
    // ============================================================================
    #[test]
    fn reads_legacy_headerless_object() -> Result<()> {
        let content = b"legacy object";
        let fixture = Fixture::new(content)?;
        let hash = format!("{:x}", Sha256::digest(content));
        let path = fixture.store.object_path(&hash)?;
        fs::create_dir_all(path.parent().expect("object path has a parent"))?;
        fs::write(path, content)?;

        let mut restored = Vec::new();
        copy_verified(&fixture.store, &hash, content.len() as u64, &mut restored)?;
        assert_eq!(restored, content);
        Ok(())
    }

    struct Fixture {
        _root: tempfile::TempDir,
        store: Store,
        file: std::path::PathBuf,
    }

    impl Fixture {
        fn new(content: &[u8]) -> Result<Self> {
            let root = tempfile::tempdir()?;
            let target = root.path().join("target");
            fs::create_dir(&target)?;
            let file = target.join("file.bin");
            fs::write(&file, content)?;
            let store = Store::init(&target, None)?;
            Ok(Self {
                _root: root,
                store,
                file,
            })
        }
    }
}
