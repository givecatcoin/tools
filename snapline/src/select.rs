//! ============================================================================
//! スナップショット内パスの絞り込みと容量集計。
//! restore / tree / find が同じ規則で使う。
//! ============================================================================

use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

use crate::model::{Entry, EntryKind, SnapshotManifest};

// ============================================================================
// 利用者指定の相対パスフィルタ。None はスナップ全体。
// ============================================================================
#[derive(Debug, Clone)]
pub struct PathFilter {
    raw: Option<PathBuf>,
}

impl PathFilter {
    // ========================================================================
    // 文字列からフィルタを作る。空や危険な成分は拒否する。
    // ========================================================================
    pub fn parse(raw: Option<&str>) -> Result<Self> {
        let Some(text) = raw.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(Self { raw: None });
        };
        let normalized = text.trim_end_matches(['/', '\\']);
        if normalized.is_empty() {
            bail!("path filter must not be empty");
        }
        for part in normalized.replace('\\', "/").split('/') {
            if part.is_empty() || part == "." || part == ".." {
                bail!(
                    "path filter must be a relative path without '.' or '..': {text}"
                );
            }
        }
        let path = PathBuf::from(normalized);
        validate_user_relative_path(&path)?;
        Ok(Self { raw: Some(path) })
    }

    // ========================================================================
    // 表示用の文字列（未指定なら None）。
    // ========================================================================
    pub fn display(&self) -> Option<String> {
        self.raw
            .as_ref()
            .map(|path| path_to_display(path))
    }

    // ========================================================================
    // エントリパスがフィルタ配下（またはフィルタ自身）か判定する。
    // `doc` が `docs` に部分一致しないよう、成分単位で見る。
    // ========================================================================
    pub fn matches(&self, path: &Path) -> bool {
        let Some(filter) = &self.raw else {
            return true;
        };
        let filter_parts: Vec<_> = filter.components().collect();
        let path_parts: Vec<_> = path.components().collect();
        if path_parts.len() < filter_parts.len() {
            return false;
        }
        path_parts[..filter_parts.len()] == filter_parts[..]
    }
}

// ============================================================================
// 絞り込んだ結果の件数とファイル論理サイズ。
// ============================================================================
#[derive(Debug, Clone, Default)]
pub struct SelectionSummary {
    pub entry_count: usize,
    pub file_count: usize,
    pub dir_count: usize,
    pub symlink_count: usize,
    pub file_bytes: u64,
}

impl SelectionSummary {
    // ========================================================================
    // エントリ列から集計する。
    // ========================================================================
    pub fn from_entries<'a, I>(entries: I) -> Self
    where
        I: IntoIterator<Item = &'a Entry>,
    {
        let mut summary = Self::default();
        for entry in entries {
            summary.entry_count += 1;
            match entry.kind {
                EntryKind::File => {
                    summary.file_count += 1;
                    summary.file_bytes = summary.file_bytes.saturating_add(entry.size);
                }
                EntryKind::Directory => summary.dir_count += 1,
                EntryKind::Symlink => summary.symlink_count += 1,
            }
        }
        summary
    }
}

// ============================================================================
// マニフェストからフィルタに合うエントリだけを返す。
// フィルタ指定時に 0 件ならエラー。
// ============================================================================
pub fn select_entries<'a>(
    manifest: &'a SnapshotManifest,
    filter: &PathFilter,
) -> Result<Vec<&'a Entry>> {
    let selected: Vec<_> = manifest
        .entries
        .iter()
        .filter(|entry| filter.matches(&entry.path))
        .collect();
    if filter.raw.is_some() && selected.is_empty() {
        bail!(
            "no entries matched path filter: {}",
            filter.display().unwrap_or_default()
        );
    }
    Ok(selected)
}

// ============================================================================
// 人が読みやすいバイト表示。
// ============================================================================
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    let value = bytes as f64;
    if value >= GIB {
        format!("{:.1} GiB", value / GIB)
    } else if value >= MIB {
        format!("{:.1} MiB", value / MIB)
    } else if value >= KIB {
        format!("{:.1} KiB", value / KIB)
    } else {
        format!("{bytes} B")
    }
}

// ============================================================================
// 表示用にパス区切りを `/` へ揃える。
// ============================================================================
pub fn path_to_display(path: &Path) -> String {
    path.components()
        .filter_map(|part| match part {
            Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

// ============================================================================
// 利用者入力の相対パスを検査する。
// ============================================================================
fn validate_user_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty() {
        bail!("path filter must not be empty");
    }
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("path filter must be a relative path without '.' or '..': {}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{PathFilter, format_bytes, path_to_display};
    use std::path::Path;

    // ============================================================================
    // prefix 一致が成分単位であることを確認する。
    // ============================================================================
    #[test]
    fn filter_matches_prefix_by_components() {
        let filter = PathFilter::parse(Some("docs")).unwrap();
        assert!(filter.matches(Path::new("docs")));
        assert!(filter.matches(Path::new("docs/a.txt")));
        assert!(!filter.matches(Path::new("docsx/a.txt")));
        assert!(!filter.matches(Path::new("other/docs/a.txt")));
    }

    // ============================================================================
    // 危険なフィルタを拒否することを確認する。
    // ============================================================================
    #[test]
    fn rejects_unsafe_filters() {
        assert!(PathFilter::parse(Some("..")).is_err());
        assert!(PathFilter::parse(Some("/abs")).is_err());
        assert!(PathFilter::parse(Some("a/./b")).is_err());
        assert!(PathFilter::parse(Some(r"a\..\b")).is_err());
    }

    // ============================================================================
    // バイト表示とパス表示の体裁を確認する。
    // ============================================================================
    #[test]
    fn formats_bytes_and_paths() {
        assert_eq!(format_bytes(512), "512 B");
        assert!(format_bytes(3 * 1024 * 1024).contains("MiB"));
        assert_eq!(
            path_to_display(&Path::new("docs").join("a.txt")).replace('\\', "/"),
            "docs/a.txt"
        );
    }
}
