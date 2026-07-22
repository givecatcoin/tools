//! ============================================================================
//! 内容アドレス化されたオブジェクトの取り込みと読み出し。
//!
//! ハッシュは「元のファイル内容」の SHA-256。
//! 圧縮して保存してもハッシュは変えない。
//! これにより、圧縮有無が違ってもスナップショット間で同じ内容を共有できる。
//!
//! オンディスク形式:
//!   SNAPOBJ1  (8 bytes magic)
//!   codec     (1 byte: 0=raw, 1=zstd)
//!   payload   (raw bytes or zstd frame)
//!
//! magic が無い旧オブジェクトは、生バイト列として読む（後方互換）。
//! ============================================================================

use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Seek, SeekFrom, Write},
    path::Path,
    time::UNIX_EPOCH,
};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::{pace::IoPace, store::Store};

/// 現行オブジェクト形式の識別子。
const MAGIC: &[u8; 8] = b"SNAPOBJ1";

/// 圧縮せずそのまま格納する。
const CODEC_RAW: u8 = 0;

/// Zstandard で圧縮して格納する。
const CODEC_ZSTD: u8 = 1;

/// 速度と比率のバランスを取った既定レベル。極端に高くしない。
const COMPRESSION_LEVEL: i32 = 3;

const BUFFER_SIZE: usize = 1024 * 1024;

// ============================================================================
// ファイルをストアへ取り込み、内容ハッシュを返す（通常優先度）。
// ============================================================================
#[allow(dead_code)] // background 経路は ingest_with_pace を使う公開ラッパ。
pub fn ingest(store: &Store, path: &Path, before: &Metadata) -> Result<String> {
    ingest_with_pace(store, path, before, &mut crate::pace::IdlePace)
}

// ============================================================================
// ペース制御付きの取り込み。通常経路は IdlePace、background のみ別実装を渡す。
// ============================================================================
pub fn ingest_with_pace(
    store: &Store,
    path: &Path,
    before: &Metadata,
    pace: &mut dyn IoPace,
) -> Result<String> {
    let mut source =
        File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(store.root.join("tmp"))?;

    // まず圧縮前提で書く。後で「小さくならなかった」と判断したら raw に作り直す。
    temp.write_all(MAGIC)?;
    temp.write_all(&[CODEC_ZSTD])?;

    let mut hasher = Sha256::new();
    {
        let mut encoder = zstd::stream::write::Encoder::new(&mut temp, COMPRESSION_LEVEL)?;
        copy_and_hash(&mut source, &mut encoder, &mut hasher, pace)
            .with_context(|| format!("failed to read {}", path.display()))?;
        encoder.finish()?;
    }

    let hash = format!("{:x}", hasher.finalize());
    let compressed_payload_size = temp.as_file().metadata()?.len() - (MAGIC.len() as u64 + 1);

    // 画像や既圧縮データなど、圧縮すると逆に膨らむものは raw で持つ。
    if compressed_payload_size >= before.len() {
        temp.as_file_mut().set_len(0)?;
        temp.as_file_mut().seek(SeekFrom::Start(0))?;
        temp.write_all(MAGIC)?;
        temp.write_all(&[CODEC_RAW])?;

        let mut second_source = File::open(path)?;
        let mut second_hasher = Sha256::new();
        copy_and_hash(&mut second_source, &mut temp, &mut second_hasher, pace)?;
        let second_hash = format!("{:x}", second_hasher.finalize());

        // 再読込中に内容が変わっていたら、壊れたオブジェクトを残さない。
        if second_hash != hash {
            bail!("file changed while being captured: {}", path.display());
        }
    }

    // メタデータの変化も「取り込み中に書き換えられた」合図として扱う。
    let after = fs::metadata(path)?;
    if before.len() != after.len() || modified_nanos(before) != modified_nanos(&after) {
        bail!("file changed while being captured: {}", path.display());
    }
    temp.as_file().sync_all()?;

    let destination = store.object_path(&hash)?;
    if !destination.exists() {
        let parent = destination
            .parent()
            .context("object path has no parent directory")?;
        fs::create_dir_all(parent)?;

        // noclobber: 競合時は既存を尊重。同じ内容のはずなので上書き不要。
        match temp.persist_noclobber(&destination) {
            Ok(_) => {}
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(error.error).with_context(|| {
                    format!("failed to persist object {}", destination.display())
                });
            }
        }
    }

    Ok(hash)
}

// ============================================================================
// オブジェクトを展開しつつハッシュ検証する（通常優先度）。
// ============================================================================
#[allow(dead_code)] // background 経路は copy_verified_with_pace を使う公開ラッパ。
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
            // magic 不一致 = 圧縮導入前の生オブジェクト。先頭から生読みする。
            file.seek(SeekFrom::Start(0))?;
            return Ok(Box::new(file));
        }
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => {
            // 空ファイルなど短すぎる場合も旧形式として扱う。
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

    use super::{CODEC_RAW, MAGIC, copy_verified, ingest};
    use crate::store::Store;

    // ============================================================================
    // 繰り返しデータが圧縮され、復元できることを確認する。
    // ============================================================================
    #[test]
    fn compresses_repetitive_content_and_restores_it() -> Result<()> {
        let fixture = Fixture::new(&vec![b'a'; 128 * 1024])?;
        let hash = ingest(&fixture.store, &fixture.file, &fs::metadata(&fixture.file)?)?;
        let stored = fs::read(fixture.store.object_path(&hash)?)?;

        assert_eq!(&stored[..MAGIC.len()], MAGIC);
        assert!(stored.len() < 1024);

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
        // 単純な xorshift で擬似乱数を作り、圧縮が効きにくい入力にする。
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

    // ============================================================================
    // テスト用に一時対象ツリーとストアを用意する。
    // ============================================================================
    struct Fixture {
        _root: tempfile::TempDir,
        store: Store,
        file: std::path::PathBuf,
    }

    impl Fixture {
        // ============================================================================
        // 指定内容の一時ファイルと、それを対象とするストアを作る。
        // ============================================================================
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
