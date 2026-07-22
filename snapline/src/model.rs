//! ============================================================================
//! オンディスクに保存するデータ形の定義。
//! ここにある型は JSON としてストアに残るため、
//! フィールドの意味を変えるときは FORMAT_VERSION と読み込み互換を意識する。
//! ============================================================================

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::settings::UserSettings;

// ============================================================================
// オンディスク形式の版。互換を壊す変更でのみ上げる。
// ============================================================================
pub const FORMAT_VERSION: u32 = 1;

// ============================================================================
// ストア全体の設定。対象パスとユーザー設定をまとめて保持する。
// 1 ストアにつき対象ディレクトリは 1 つ。
// 別の対象を履歴化したい場合は別ストアを作る。
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// この設定ファイルがどの形式版か。
    pub format_version: u32,

    /// 初期化時の対象絶対パス。ツリー移動後は実行時の指定パスで上書きして使う。
    pub target: PathBuf,

    /// 除外ルールなど、運用者が調整する項目。
    /// 旧ストアにフィールドが無い場合は既定値で補完する。
    #[serde(default = "UserSettings::defaults")]
    pub settings: UserSettings,
}

// ============================================================================
// 1 回のスナップショットの目録。
// 実ファイル本体は objects/ 側に置き、ここではパスと参照ハッシュを持つ。
// ============================================================================
#[derive(Debug, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub format_version: u32,

    /// ファイル名にも使う一意 ID（英数字・ハイフン・アンダースコアのみ）。
    pub id: String,

    /// RFC3339 形式の作成時刻。一覧表示の並び替えにも使う。
    pub created_at: String,

    /// 利用者が付けた任意メモ。無くてもよい。
    pub message: Option<String>,

    /// 対象ツリー内の各エントリ。ルート自体は含めない。
    pub entries: Vec<Entry>,
}

// ============================================================================
// `log` 用の軽い要約。マニフェスト本体とは別に summaries/ へ保存する。
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub format_version: u32,
    pub id: String,
    pub created_at: String,
    pub message: Option<String>,
    pub entry_count: usize,
}

impl SnapshotSummary {
    // ========================================================================
    // マニフェストから要約を作る。
    // ========================================================================
    pub fn from_manifest(manifest: &SnapshotManifest) -> Self {
        Self {
            format_version: FORMAT_VERSION,
            id: manifest.id.clone(),
            created_at: manifest.created_at.clone(),
            message: manifest.message.clone(),
            entry_count: manifest.entries.len(),
        }
    }
}

// ============================================================================
// スナップショット内の 1 エントリ（対象ルートからの相対パス基準）。
// ============================================================================
#[derive(Debug, Serialize, Deserialize)]
pub struct Entry {
    /// 対象ルートからの相対パス。絶対パスや `..` を含めてはならない。
    pub path: PathBuf,

    pub kind: EntryKind,

    /// ファイルのときだけ、内容の SHA-256（小文字 hex）。
    pub object: Option<String>,

    /// 元ファイルのバイト数。整合性検証でも使う。
    pub size: u64,

    /// 更新時刻（UNIX epoch からのナノ秒）。取得できない環境では None。
    pub modified_unix_nanos: Option<u128>,

    /// 読み取り専用フラグ。プラットフォーム共通で扱える範囲に限定する。
    pub readonly: bool,

    /// シンボリックリンクのリンク先（未解決のまま保存）。
    pub symlink_target: Option<PathBuf>,

    /// Windows 復元時に dir/file どちらでリンクを作るかのヒント。
    pub symlink_is_dir: bool,
}

// ============================================================================
// ファイルシステム上のエントリ種別。
// ============================================================================
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
}

#[cfg(test)]
mod tests {
    use super::StoreConfig;

    // ============================================================================
    // 最小構成の config.json を読めることを確認する。
    // ============================================================================
    #[test]
    fn store_config_deserializes_minimal_fields() {
        let json = r#"{
            "format_version": 1,
            "target": "C:/work",
            "settings": {
                "exclude_dir_names": [],
                "exclude_file_names": [],
                "exclude_extensions": [],
                "protect_git": true
            }
        }"#;
        let config: StoreConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format_version, 1);
        assert_eq!(config.target, std::path::PathBuf::from("C:/work"));
        assert!(config.settings.protect_git);
    }

    // ============================================================================
    // 未知フィールドがあっても読めることを確認する。
    // ============================================================================
    #[test]
    fn store_config_ignores_unknown_fields() {
        let json = r#"{
            "format_version": 1,
            "target": "C:/work",
            "settings": {
                "exclude_dir_names": [],
                "exclude_file_names": [],
                "exclude_extensions": [],
                "protect_git": true
            },
            "experimental_note": "ignored"
        }"#;
        let config: StoreConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.format_version, 1);
    }
}
