//! ============================================================================
//! スナップショット内容の閲覧（tree / find）。
//! 復元前の下調べ用。書き込みはしない。
//! ============================================================================

use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use anyhow::{Result, bail};

use crate::{
    model::EntryKind,
    progress::Progress,
    select::{PathFilter, SelectionSummary, path_to_display, select_entries},
    store::Store,
};

// ============================================================================
// tree 表示の結果。
// ============================================================================
pub struct TreeOutcome {
    pub lines: Vec<String>,
    pub summary: SelectionSummary,
    pub filter: Option<String>,
}

// ============================================================================
// find 表示の結果。
// ============================================================================
pub struct FindOutcome {
    pub lines: Vec<String>,
    pub summary: SelectionSummary,
    pub pattern: String,
}

// ============================================================================
// スナップ内のパスをツリー表示する。
// depth は表示する最大の深さ（1 なら直下まで）。None は制限なし。
// ============================================================================
pub fn tree(
    store: &Store,
    id: &str,
    path: Option<&str>,
    depth: Option<usize>,
    progress: &mut Progress,
) -> Result<TreeOutcome> {
    progress.begin("Reading snapshot");
    let manifest = store.read_manifest(id)?;
    crate::restore::validate_manifest(&manifest)?;
    progress.done(&format!("{} entries in manifest, done.", manifest.entries.len()));

    progress.begin("Building tree");
    let filter = PathFilter::parse(path)?;
    let selected = select_entries(&manifest, &filter)?;
    let summary = SelectionSummary::from_entries(selected.iter().copied());
    let lines = render_tree(&selected, filter.display().as_deref(), depth);
    progress.done(&format!("{} lines, done.", lines.len()));

    Ok(TreeOutcome {
        lines,
        summary,
        filter: filter.display(),
    })
}

// ============================================================================
// スナップ内パスを部分一致で探す（大小無視、`/` 区切りで比較）。
// ============================================================================
pub fn find(
    store: &Store,
    id: &str,
    pattern: &str,
    progress: &mut Progress,
) -> Result<FindOutcome> {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        bail!("find pattern must not be empty");
    }

    progress.begin("Reading snapshot");
    let manifest = store.read_manifest(id)?;
    crate::restore::validate_manifest(&manifest)?;
    progress.done(&format!("{} entries in manifest, done.", manifest.entries.len()));

    progress.begin("Searching paths");
    let needle = pattern.to_ascii_lowercase();
    let mut matched = Vec::new();
    let total = manifest.entries.len().max(1);
    for (index, entry) in manifest.entries.iter().enumerate() {
        let display = path_to_display(&entry.path);
        if display.to_ascii_lowercase().contains(&needle) {
            matched.push(entry);
        }
        progress.ratio(index + 1, total);
    }

    let summary = SelectionSummary::from_entries(matched.iter().copied());
    let mut lines: Vec<String> = matched
        .iter()
        .map(|entry| {
            let kind = match entry.kind {
                EntryKind::Directory => "dir",
                EntryKind::File => "file",
                EntryKind::Symlink => "symlink",
            };
            format!("{kind}\t{}", path_to_display(&entry.path))
        })
        .collect();
    lines.sort();
    progress.done(&format!("{} matches, done.", lines.len()));

    Ok(FindOutcome {
        lines,
        summary,
        pattern: pattern.to_owned(),
    })
}

// ============================================================================
// 選択エントリからインデント付きツリー行を作る。
// ============================================================================
fn render_tree(entries: &[&crate::model::Entry], filter: Option<&str>, depth: Option<usize>) -> Vec<String> {
    let mut root = TreeNode::default();
    for entry in entries {
        let relative = strip_filter_prefix(&entry.path, filter);
        let comps: Vec<_> = relative
            .components()
            .filter_map(|part| match part {
                Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect();
        if comps.is_empty() {
            // フィルタ自身がディレクトリ等のとき、ルート見出しだけ出す。
            continue;
        }
        if let Some(max) = depth
            && comps.len() > max
        {
            continue;
        }
        root.insert(&comps, entry.kind, entry.size);
    }

    let mut lines = Vec::new();
    if let Some(label) = filter {
        lines.push(format!("{label}/"));
        root.render_into(&mut lines, 1);
    } else {
        root.render_into(&mut lines, 0);
    }
    lines
}

// ============================================================================
// フィルタ prefix を除いた相対パスを返す。
// ============================================================================
fn strip_filter_prefix(path: &Path, filter: Option<&str>) -> std::path::PathBuf {
    let Some(filter) = filter else {
        return path.to_path_buf();
    };
    let filter_path = Path::new(filter);
    path.strip_prefix(filter_path)
        .map(|rest| rest.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
}

#[derive(Default)]
struct TreeNode {
    children: BTreeMap<String, TreeNode>,
    kind: Option<EntryKind>,
    size: u64,
}

impl TreeNode {
    fn insert(&mut self, parts: &[String], kind: EntryKind, size: u64) {
        if parts.is_empty() {
            return;
        }
        let child = self.children.entry(parts[0].clone()).or_default();
        if parts.len() == 1 {
            child.kind = Some(kind);
            child.size = size;
        } else {
            child.insert(&parts[1..], kind, size);
        }
    }

    fn render_into(&self, lines: &mut Vec<String>, depth: usize) {
        for (name, child) in &self.children {
            let indent = "  ".repeat(depth);
            let is_dir =
                child.kind == Some(EntryKind::Directory) || !child.children.is_empty();
            let line = if is_dir {
                format!("{indent}{name}/")
            } else if child.kind == Some(EntryKind::Symlink) {
                format!("{indent}{name}  ->")
            } else {
                format!(
                    "{indent}{name}  ({})",
                    crate::select::format_bytes(child.size)
                )
            };
            lines.push(line);
            child.render_into(lines, depth + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::render_tree;
    use crate::model::{Entry, EntryKind};
    use std::path::PathBuf;

    fn entry(path: &str, kind: EntryKind, size: u64) -> Entry {
        Entry {
            path: PathBuf::from(path),
            kind,
            object: None,
            size,
            modified_unix_nanos: None,
            readonly: false,
            symlink_target: None,
            symlink_is_dir: false,
        }
    }

    // ============================================================================
    // ツリー行に子パスがインデント付きで出ることを確認する。
    // ============================================================================
    #[test]
    fn renders_indented_tree() {
        let docs = entry("docs", EntryKind::Directory, 0);
        let readme = entry("docs/readme.txt", EntryKind::File, 100);
        let selected = vec![&docs, &readme];
        let lines = render_tree(&selected, Some("docs"), None);
        assert!(lines.iter().any(|line| line == "docs/"));
        assert!(lines.iter().any(|line| line.contains("readme.txt")));
    }
}
