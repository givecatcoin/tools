//! ============================================================================
//! `.snaplineignore` による除外ルール。
//! `.gitignore` と同じ記法で、各ディレクトリに置ける。
//! 祖先から当該ディレクトリまでのすべての `.snaplineignore` を重ねて適用する。
//! ============================================================================

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};

pub const SNAPLINEIGNORE_FILE: &str = ".snaplineignore";

// ============================================================================
// 対象ツリー内の `.snaplineignore` を遅延ロードし、パス判定に使う。
// ============================================================================
pub struct SnaplineignoreMatcher {
    target: PathBuf,
    /// ディレクトリ絶対パス → その場所に置かれたルール。
    layers: HashMap<PathBuf, Option<Gitignore>>,
}

impl SnaplineignoreMatcher {
    // ============================================================================
    // 対象ツリーを受け取り、空のキャッシュで始める。
    // ============================================================================
    pub fn new(target: &Path) -> Self {
        Self {
            target: target.to_path_buf(),
            layers: HashMap::new(),
        }
    }

    // ============================================================================
    // このエントリをスナップショットから除外するか判定する。
    // ============================================================================
    pub fn should_exclude(&mut self, absolute: &Path, is_dir: bool) -> Result<bool> {
        let relative = absolute
            .strip_prefix(&self.target)
            .with_context(|| format!("path escaped target: {}", absolute.display()))?
            .to_path_buf();

        // 先に必要なレイヤーをすべてロードし、その後で参照だけする。
        // （ロード中の self 可変借用と判定時の不変借用がぶつからないようにする）
        let mut dirs = vec![self.target.clone()];
        let mut prefix = PathBuf::new();
        let components: Vec<_> = relative.components().collect();
        for (index, component) in components.iter().enumerate() {
            if index + 1 == components.len() {
                break;
            }
            prefix.push(component);
            dirs.push(self.target.join(&prefix));
        }
        for dir in &dirs {
            self.ensure_layer(dir)?;
        }

        let mut ignored = false;
        let mut layer_prefix = PathBuf::new();
        for (index, dir) in dirs.iter().enumerate() {
            let path_from_layer = if index == 0 {
                relative.as_path()
            } else {
                layer_prefix.push(components[index - 1]);
                relative
                    .strip_prefix(&layer_prefix)
                    .unwrap_or(relative.as_path())
            };
            if let Some(rules) = self.layers.get(dir).and_then(|value| value.as_ref()) {
                ignored = apply_match(rules, path_from_layer, is_dir, ignored);
            }
        }

        Ok(ignored)
    }

    // ============================================================================
    // 指定ディレクトリの `.snaplineignore` を読み、無ければ None をキャッシュする。
    // ============================================================================
    fn ensure_layer(&mut self, dir: &Path) -> Result<()> {
        if self.layers.contains_key(dir) {
            return Ok(());
        }

        let file = dir.join(SNAPLINEIGNORE_FILE);
        let layer = if file.is_file() {
            let mut builder = GitignoreBuilder::new(dir);
            let error = builder.add(&file);
            if let Some(error) = error {
                return Err(error).with_context(|| format!("failed to read {}", file.display()));
            }
            Some(
                builder
                    .build()
                    .with_context(|| format!("failed to parse {}", file.display()))?,
            )
        } else {
            None
        };
        self.layers.insert(dir.to_path_buf(), layer);
        Ok(())
    }
}

// ============================================================================
// 1 レイヤーの判定結果を、これまでの ignored 状態へ反映する。
// ============================================================================
fn apply_match(rules: &Gitignore, path: &Path, is_dir: bool, ignored: bool) -> bool {
    let matched = rules.matched(path, is_dir);
    if matched.is_ignore() {
        true
    } else if matched.is_whitelist() {
        false
    } else {
        ignored
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use super::SnaplineignoreMatcher;

    // ============================================================================
    // ルートと子階層の `.snaplineignore` が両方効くことを確認する。
    // ============================================================================
    #[test]
    fn applies_nested_snaplineignore_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("tree");
        fs::create_dir_all(target.join("project/logs"))?;
        fs::write(target.join(".snaplineignore"), "*.tmp\n")?;
        fs::write(target.join("project/.snaplineignore"), "logs/\n")?;
        fs::write(target.join("keep.txt"), "ok")?;
        fs::write(target.join("drop.tmp"), "x")?;
        fs::write(target.join("project/logs/a.txt"), "x")?;
        fs::write(target.join("project/ok.txt"), "ok")?;

        let mut matcher = SnaplineignoreMatcher::new(&target);
        assert!(!matcher.should_exclude(&target.join("keep.txt"), false)?);
        assert!(matcher.should_exclude(&target.join("drop.tmp"), false)?);
        assert!(matcher.should_exclude(&target.join("project/logs"), true)?);
        assert!(!matcher.should_exclude(&target.join("project/ok.txt"), false)?);
        Ok(())
    }

    // ============================================================================
    // `.snaplineignore` で `.git` を除外できることを確認する。
    // ============================================================================
    #[test]
    fn snaplineignore_can_exclude_dot_git() -> Result<()> {
        let root = tempfile::tempdir()?;
        let target = root.path().join("tree");
        fs::create_dir_all(target.join("repo/.git"))?;
        fs::write(target.join(".snaplineignore"), ".git/\n")?;

        let mut matcher = SnaplineignoreMatcher::new(&target);
        assert!(matcher.should_exclude(&target.join("repo/.git"), true)?);
        Ok(())
    }
}
