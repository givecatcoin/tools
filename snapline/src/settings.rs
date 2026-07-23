//! ============================================================================
//! ユーザー設定と既定値の方針をまとめる。
//!
//! Snapline の除外は `.gitignore` に追従しない。
//! `.gitignore` は「共有しないもの」の一覧であり、「履歴に残さないもの」ではない。
//! `.env` やローカル設定など、Git に無いからこそ保全したいファイルを
//! 誤って落とす危険がある。
//!
//! 代わりに Snapline 専用の `.snaplinenore`（gitignore 互換記法）を使う。
//! 既定のディレクトリ名除外は、その上に常に効く安全側の土台である。
//! ファイル名・拡張子除外は既定で空とし、必要なときだけ明示追加する。
//! ============================================================================

use std::{ffi::OsStr, path::Path};

use serde::{Deserialize, Serialize};

// ============================================================================
// ストアに保存するユーザー向け設定。
// init 時に既定値が書き込まれ、以後は config.json を編集して調整する。
// 古いストアにフィールドが無い場合は、serde の default で現行の既定値を補完する。
// ============================================================================
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSettings {
    /// ツリー内のどこにあっても、この名前のディレクトリは記録対象外とする。
    /// マッチは「パス成分のディレクトリ名」のみ。ファイル名は対象にしない。
    /// 例: `src/target/debug` は `target` が一致すれば配下ごと除外される。
    #[serde(default = "default_exclude_dir_names")]
    pub exclude_dir_names: Vec<String>,

    /// ツリー内のどこにあっても、この名前のファイルは記録対象外とする。
    /// ディレクトリには適用しない。既定は空（明示したときだけ除外）。
    #[serde(default)]
    pub exclude_file_names: Vec<String>,

    /// この拡張子のファイルは記録対象外とする。
    /// 先頭の `.` 有無は問わない（`log` と `.log` は同じ）。
    /// 複合拡張子は末尾だけを見る（`archive.tar.gz` は `gz`）。既定は空。
    #[serde(default)]
    pub exclude_extensions: Vec<String>,
}

impl UserSettings {
    // ============================================================================
    // 新規ストア向けの既定設定を返す。
    // ============================================================================
    pub fn defaults() -> Self {
        Self {
            exclude_dir_names: default_exclude_dir_names(),
            exclude_file_names: Vec::new(),
            exclude_extensions: Vec::new(),
        }
    }

    // ============================================================================
    // このディレクトリ名をスナップショットから除外するか判定する。
    // ============================================================================
    pub fn should_exclude_dir_name(&self, name: &OsStr) -> bool {
        let Some(name) = name.to_str() else {
            // 非 Unicode 名は除外ルールの対象外とし、記録側で扱う。
            return false;
        };

        self.exclude_dir_names
            .iter()
            .any(|pattern| names_equal(name, pattern))
    }

    // ============================================================================
    // このファイル名（パス末尾）を除外するか判定する。ディレクトリには使わない。
    // ============================================================================
    pub fn should_exclude_file_name(&self, name: &OsStr) -> bool {
        if self.exclude_file_names.is_empty() {
            return false;
        }
        let Some(name) = name.to_str() else {
            return false;
        };
        self.exclude_file_names
            .iter()
            .any(|pattern| names_equal(name, pattern))
    }

    // ============================================================================
    // このパスの拡張子が除外対象か判定する。ディレクトリには使わない。
    // ============================================================================
    pub fn should_exclude_extension(&self, path: &Path) -> bool {
        if self.exclude_extensions.is_empty() {
            return false;
        }
        let Some(extension) = path.extension().and_then(OsStr::to_str) else {
            return false;
        };
        self.exclude_extensions
            .iter()
            .any(|pattern| names_equal(extension, normalize_extension(pattern)))
    }

    // ============================================================================
    // ファイルエントリを設定のファイル名・拡張子ルールで除外するか。
    // ============================================================================
    pub fn should_exclude_file(&self, path: &Path) -> bool {
        let name = path
            .file_name()
            .map(OsStr::to_owned)
            .unwrap_or_else(|| OsStr::new("").to_owned());
        self.should_exclude_file_name(&name) || self.should_exclude_extension(path)
    }
}

// ============================================================================
// 既定で除外するディレクトリ名。
//
// 選定方針:
// 1. 再取得・再生成できる依存木・キャッシュ・ビルド成果物に限る
// 2. Git 関連（.git / .gitignore / .gitattributes など）は載せない
// 3. 秘密情報や作業データ（.env、secrets、data など）は載せない
// 4. 名前が短く誤爆しやすいもの（bin、tmp、out など）は載せない
// 5. 迷うものは含めず、ユーザーが exclude_dir_names へ明示追加する
//
// dist / build / target は容量影響が大きいため既定に含める。
// 成果物そのものを履歴したい場合は、該当名を設定から削除する。
// ============================================================================
pub fn default_exclude_dir_names() -> Vec<String> {
    [
        // --- パッケージ依存（巨大かつ再取得可能） ---
        "node_modules",
        "bower_components",
        // --- Rust ---
        "target",
        // --- Python ---
        "__pycache__",
        ".venv",
        "venv",
        ".tox",
        ".mypy_cache",
        ".pytest_cache",
        ".ruff_cache",
        // --- JS/TS ツールチェーンのキャッシュ ---
        ".next",
        ".nuxt",
        ".parcel-cache",
        ".turbo",
        ".cache",
        // --- よくあるビルド / カバレッジ出力 ---
        "dist",
        "build",
        "coverage",
        // --- モバイル / JVM ---
        "Pods",
        ".gradle",
        // --- Dart ---
        ".dart_tool",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

// ============================================================================
// 拡張子指定の先頭ドットを除く。空になった場合はそのまま返す。
// ============================================================================
fn normalize_extension(value: &str) -> &str {
    value.strip_prefix('.').unwrap_or(value)
}

// ============================================================================
// 名前比較。
// Windows は大文字小文字を区別しないファイルシステムが一般的なため、
// そこでは ASCII の大文字小文字を無視する。それ以外は厳密一致。
// ============================================================================
fn names_equal(left: &str, right: &str) -> bool {
    if cfg!(windows) {
        left.eq_ignore_ascii_case(right)
    } else {
        left == right
    }
}

#[cfg(test)]
mod tests {
    use std::{ffi::OsStr, path::Path};

    use super::{UserSettings, default_exclude_dir_names};

    // ============================================================================
    // 既定除外に Git 関連名が含まれないことを確認する。
    // ============================================================================
    #[test]
    fn defaults_never_exclude_git_related_names() {
        let names = default_exclude_dir_names();
        for forbidden in [".git", ".gitignore", ".gitattributes", ".gitmodules"] {
            assert!(
                !names.iter().any(|name| name == forbidden),
                "{forbidden} must not be in default excludes"
            );
        }
    }

    // ============================================================================
    // 既定では .git を除外しないことを確認する。
    // ============================================================================
    #[test]
    fn defaults_keep_dot_git_directory() {
        let settings = UserSettings::defaults();
        assert!(!settings.should_exclude_dir_name(OsStr::new(".git")));
        assert!(settings.should_exclude_dir_name(OsStr::new("node_modules")));
    }

    // ============================================================================
    // 明示追加すれば .git を除外できることを確認する。
    // ============================================================================
    #[test]
    fn can_exclude_dot_git_by_adding_to_exclude_list() {
        let settings = UserSettings {
            exclude_dir_names: vec![".git".into(), "node_modules".into()],
            exclude_file_names: Vec::new(),
            exclude_extensions: Vec::new(),
        };
        assert!(settings.should_exclude_dir_name(OsStr::new(".git")));
        assert!(settings.should_exclude_dir_name(OsStr::new("node_modules")));
    }

    // ============================================================================
    // 判定は名前のみであり、呼び出し側がディレクトリに限って使う前提を確認する。
    // ============================================================================
    #[test]
    fn does_not_treat_files_by_path_semantics_here() {
        let settings = UserSettings::defaults();
        assert!(settings.should_exclude_dir_name(OsStr::new("target")));
        assert!(!settings.should_exclude_dir_name(OsStr::new("Cargo.toml")));
    }

    // ============================================================================
    // ファイル名除外は完全一致で、ディレクトリ名除外とは独立であることを確認する。
    // ============================================================================
    #[test]
    fn excludes_configured_file_names() {
        let settings = UserSettings {
            exclude_dir_names: Vec::new(),
            exclude_file_names: vec!["Thumbs.db".into(), "desktop.ini".into()],
            exclude_extensions: Vec::new(),
        };
        assert!(settings.should_exclude_file(Path::new("photos/Thumbs.db")));
        assert!(settings.should_exclude_file(Path::new("desktop.ini")));
        assert!(!settings.should_exclude_file(Path::new("readme.txt")));
        assert!(!settings.should_exclude_dir_name(OsStr::new("Thumbs.db")));
    }

    // ============================================================================
    // 拡張子除外は先頭ドットの有無を同一視することを確認する。
    // ============================================================================
    #[test]
    fn excludes_configured_extensions() {
        let settings = UserSettings {
            exclude_dir_names: Vec::new(),
            exclude_file_names: Vec::new(),
            exclude_extensions: vec![".log".into(), "tmp".into()],
        };
        assert!(settings.should_exclude_file(Path::new("app/noise.log")));
        assert!(settings.should_exclude_file(Path::new("cache/x.tmp")));
        assert!(!settings.should_exclude_file(Path::new("app/keep.txt")));
        assert!(!settings.should_exclude_file(Path::new("Makefile")));
    }
}
