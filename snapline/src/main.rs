//! ============================================================================
//! CLI 入口。ユーザー操作を各モジュールへ橋渡しする。
//! ============================================================================

mod background;
mod browse;
mod care;
mod inspect;
mod install;
mod model;
mod object;
mod pace;
mod progress;
mod restore;
mod select;
mod settings;
mod snaplineignore;
mod snapshot;
mod store;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::{
    background::{BackgroundLimits, BackgroundPace, activate},
    pace::{IdlePace, IoPace},
    progress::Progress,
    store::Store,
};

/// トップレベル `--help` 用。コマンドごとに使えるオプションを紐づけて載せる。
/// よくある使い方の案内は載せない。
const AFTER_HELP: &str = "\
Commands:
  init [TARGET] [--config-only] [--force]
      Create a history store for a target directory
      Options:
        --config-only
            Write only config.json into an existing store
        --force
            Allow overwriting an existing config.json (with --config-only)

  snap [-m, --message <MESSAGE>] [--rehash] [--compress]
      Capture the target tree (reuses unchanged files; stores raw by default)
      Options:
        -m, --message <MESSAGE>
            Optional note stored with this snap
        --rehash
            Reread every file and recompute content hashes (disables reuse)
        --compress
            Try zstd when ingesting new or changed files

  log [-n, --max-count <N>]
      List snapshots from oldest to newest
      Options:
        -n, --max-count <N>
            Show only the newest N snapshots

  tree <SNAPSHOT_ID> [--path <REL>] [--depth <N>]
      Show paths inside a snapshot as a tree
      Options:
        --path <REL>
            Limit to this relative path and its descendants
        --depth <N>
            Limit displayed depth under the root or --path

  find <SNAPSHOT_ID> <PATTERN>
      Find snapshot paths containing PATTERN (case-insensitive)

  restore <SNAPSHOT_ID> <DESTINATION> [--path <REL>] [--dry-run]
      Restore into a new or empty directory (never overwrites existing trees)
      Options:
        --path <REL>
            Restore only this relative path and its descendants
        --dry-run
            Plan only: show entry counts and estimated size, write nothing

  verify
      Check that stored manifests and objects are intact

  care
      Run verify, then compress objects that benefit from zstd

  config
      Show the current store configuration

  install
      Install snapline onto the user PATH

Global options (any command):
  --target <TARGET>
      Target tree root (discovered from the current directory when omitted)
  --store <STORE>
      Store location (defaults to <target>/.snapline)
  --background
      Low priority with resource-aware pacing (snap, care, restore, verify only)
  --cpu-busy-percent <CPU_BUSY_PERCENT>
      Wait while total CPU usage exceeds this percent (requires --background)
  --memory-load-percent <MEMORY_LOAD_PERCENT>
      Wait while memory load exceeds this percent (requires --background)
  --poll-ms <POLL_MS>
      Milliseconds between resource checks while waiting (requires --background)
  -h, --help
      Print help
  -V, --version
      Print version
";

// ============================================================================
// 共通オプションとサブコマンドをまとめた CLI 定義。
// `--target` は履歴対象ツリー。省略時はカレントから親方向へ `.snapline` を探す。
// `--store` は任意のストア配置先。
// `--background` は重い操作を低優先度・資源監視付きで進める。
// トップレベル `--help` は clap の自動一覧を出さず、AFTER_HELP を本体にする。
// 引数なし起動は簡略表示にする。
// ============================================================================
#[derive(Debug, Parser)]
#[command(
    name = "snapline",
    bin_name = "snapline",
    version,
    about = "Safe, content-addressed directory snapshots without Git semantics",
    after_help = AFTER_HELP,
    help_template = "\
{about-with-newline}\n\
{usage-heading} {usage}\n\
\n\
{after-help}\
"
)]
struct Cli {
    /// Target tree root directory. Discovered from the current directory when omitted.
    #[arg(long, global = true, env = "SNAPLINE_TARGET")]
    target: Option<PathBuf>,

    /// Optional store location. Defaults to <target>/.snapline.
    #[arg(long, global = true, env = "SNAPLINE_STORE")]
    store: Option<PathBuf>,

    /// Run with low priority and resource-aware pacing.
    #[arg(long, global = true)]
    background: bool,

    #[command(flatten)]
    limits: BackgroundLimitsArgs,

    #[command(subcommand)]
    command: Command,
}

// ============================================================================
// 利用者が明示的に選ぶ操作。
// clap の help 文言は英字のままにし、コード本体の説明は日本語コメントで補う。
// ============================================================================
#[derive(Debug, Subcommand)]
enum Command {
    /// Create a new history store for a target directory.
    Init {
        #[arg(value_name = "TARGET")]
        target: Option<PathBuf>,
        /// Write only config.json into an existing store.
        #[arg(long)]
        config_only: bool,
        /// Allow overwriting an existing config.json (with --config-only).
        #[arg(long)]
        force: bool,
    },
    /// Capture the current target directory (fast path by default).
    Snap {
        /// Optional note stored with this snap.
        #[arg(short, long)]
        message: Option<String>,

        /// Reread every file and recompute content hashes. Disables reuse.
        #[arg(long)]
        rehash: bool,

        /// Try zstd compression when ingesting new or changed files.
        #[arg(long)]
        compress: bool,
    },
    /// Show snapshots from oldest to newest.
    Log {
        /// Show only the newest N snapshots (e.g. `-1` for the latest only).
        #[arg(short = 'n', long = "max-count", value_name = "N", value_parser = parse_max_count)]
        max_count: Option<usize>,
    },
    /// Show paths inside a snapshot as a tree.
    Tree {
        #[arg(value_name = "SNAPSHOT_ID")]
        id: String,
        /// Limit to this relative path and its descendants.
        #[arg(long, value_name = "REL")]
        path: Option<String>,
        /// Limit displayed depth under the root or --path.
        #[arg(long, value_name = "N", value_parser = parse_max_count)]
        depth: Option<usize>,
    },
    /// Find snapshot paths containing a pattern.
    Find {
        #[arg(value_name = "SNAPSHOT_ID")]
        id: String,
        #[arg(value_name = "PATTERN")]
        pattern: String,
    },
    /// Restore a snapshot into a new or empty directory.
    Restore {
        #[arg(value_name = "SNAPSHOT_ID")]
        id: String,
        #[arg(value_name = "DESTINATION")]
        destination: PathBuf,
        /// Restore only this relative path and its descendants.
        #[arg(long, value_name = "REL")]
        path: Option<String>,
        /// Plan only: show counts and estimated size, write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Verify every referenced object in the store.
    Verify,
    /// Verify the store, then compress objects that benefit from zstd.
    Care,
    /// Show the current store configuration.
    Config,
    /// Install snapline onto the user PATH so it can be run without a full path.
    Install,
}

// ============================================================================
// バックグラウンド実行の資源しきい値。
// `--background` と一緒に使う。単独では意味を持たない。
// ============================================================================
#[derive(Debug, Clone, clap::Args)]
struct BackgroundLimitsArgs {
    /// Wait while total CPU usage exceeds this percent (1..=100). Requires --background.
    #[arg(long, global = true, default_value_t = background::DEFAULT_CPU_BUSY_PERCENT)]
    cpu_busy_percent: u8,

    /// Wait while physical memory load exceeds this percent (1..=100). Requires --background.
    #[arg(long, global = true, default_value_t = background::DEFAULT_MEMORY_LOAD_PERCENT)]
    memory_load_percent: u8,

    /// Milliseconds between resource checks while waiting. Requires --background.
    #[arg(long, global = true, default_value = "200")]
    poll_ms: u64,
}

impl BackgroundLimitsArgs {
    fn into_limits(self) -> BackgroundLimits {
        BackgroundLimits {
            cpu_busy_percent: self.cpu_busy_percent,
            memory_load_percent: self.memory_load_percent,
            poll_interval: Duration::from_millis(self.poll_ms),
        }
    }
}

// ============================================================================
// 既存ストアを使うコマンドの対象ツリーを決める。
// ============================================================================
fn resolve_target(cli: &Cli) -> Result<PathBuf> {
    match &cli.target {
        Some(target) => Ok(target.clone()),
        None => store::discover_tree_root(&std::env::current_dir()?),
    }
}

// ============================================================================
// 一覧表示用の短縮 ID を返す。
// ============================================================================
fn short_snapshot_id(id: &str) -> &str {
    let suffix = id.rsplit_once('-').map_or(id, |(_, suffix)| suffix);
    &suffix[..suffix.len().min(12)]
}

// ============================================================================
// 経過時間を人が読みやすい短い文字列にする。
// ============================================================================
fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 10.0 {
        format!("{secs:.2}s")
    } else if secs < 60.0 {
        format!("{secs:.1}s")
    } else {
        let minutes = (secs / 60.0).floor() as u64;
        let rem = secs - (minutes as f64 * 60.0);
        format!("{minutes}m{rem:04.1}s")
    }
}

// ============================================================================
// スキップされた壊れたスナップがあればエラーにする。
// ============================================================================
fn reject_if_skipped_snapshots(skipped: &[String]) -> Result<()> {
    if skipped.is_empty() {
        return Ok(());
    }
    bail!(
        "cannot complete: {} unreadable snapshot manifest(s): {}",
        skipped.len(),
        skipped.join(", ")
    )
}

// ============================================================================
// `log -1` のような git 風指定を `--max-count=N` へ正規化する。
// ============================================================================
fn normalize_log_digit_limits<I, T>(args: I) -> Vec<std::ffi::OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<std::ffi::OsString>,
{
    let mut args: Vec<std::ffi::OsString> = args.into_iter().map(Into::into).collect();
    let Some(log_at) = args.iter().position(|arg| arg == "log") else {
        return args;
    };
    for arg in &mut args[log_at + 1..] {
        let Some(text) = arg.to_str() else {
            continue;
        };
        let Some(digits) = text.strip_prefix('-') else {
            continue;
        };
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            continue;
        }
        *arg = format!("--max-count={digits}").into();
    }
    args
}

// ============================================================================
// log の件数上限を解釈する。1 以上のみ許可する。
// ============================================================================
fn parse_max_count(raw: &str) -> Result<usize, String> {
    let value: usize = raw
        .parse()
        .map_err(|_| format!("invalid max-count: {raw}"))?;
    if value == 0 {
        return Err("max-count must be at least 1".into());
    }
    Ok(value)
}

// ============================================================================
// `--background` が使える操作かどうかを検査する。
// 使えない操作に付けた場合はエラー（無視して続行しない）。
// ============================================================================
fn ensure_background_allowed(cli: &Cli) -> Result<()> {
    if !cli.background {
        return Ok(());
    }
    match cli.command {
        Command::Snap { .. } | Command::Restore { .. } | Command::Verify | Command::Care => Ok(()),
        Command::Init { .. }
        | Command::Log { .. }
        | Command::Tree { .. }
        | Command::Find { .. }
        | Command::Config
        | Command::Install => {
            bail!("--background applies only to snap, care, restore, and verify")
        }
    }
}

// ============================================================================
// ペース実装を用意する。通常は IdlePace、`--background` 時だけ監視付き。
// ============================================================================
enum PreparedPace {
    Idle(IdlePace),
    Background(BackgroundPace),
}

impl PreparedPace {
    fn as_mut(&mut self) -> &mut dyn IoPace {
        match self {
            Self::Idle(pace) => pace,
            Self::Background(pace) => pace,
        }
    }
}

fn prepare_pace(cli: &Cli) -> Result<PreparedPace> {
    if cli.background {
        Ok(PreparedPace::Background(activate(
            cli.limits.clone().into_limits(),
        )?))
    } else {
        Ok(PreparedPace::Idle(IdlePace))
    }
}

// ============================================================================
// 引数なし起動時の簡略表示本文。`--help` の詳細一覧とは分ける。
// ============================================================================
fn brief_usage_text() -> String {
    format!(
        "snapline {}\n\
         Usage: snapline <COMMAND>\n\
         Commands: init snap log tree find restore verify care config install\n\
         Try `snapline --help` for all options.",
        env!("CARGO_PKG_VERSION")
    )
}

// ============================================================================
// 引数なし起動時の簡略表示を stdout へ出す。
// ============================================================================
fn print_brief_usage() {
    println!("{}", brief_usage_text());
}

// ============================================================================
// CLI を解釈し、対応する処理へ振り分ける。
// ============================================================================
fn main() -> Result<()> {
    let raw_args: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if raw_args.len() <= 1 {
        print_brief_usage();
        return Ok(());
    }

    let cli = Cli::parse_from(normalize_log_digit_limits(raw_args));
    ensure_background_allowed(&cli)?;
    let store_opt = cli.store.as_deref();

    match &cli.command {
        Command::Init {
            target,
            config_only,
            force,
        } => {
            let mut progress = Progress::stderr_for_cli();
            let current = std::env::current_dir()?;
            let target_path = target
                .as_deref()
                .or(cli.target.as_deref())
                .unwrap_or(current.as_path());
            if *config_only {
                progress.begin("Writing config.json");
                let store = Store::init_config_only(target_path, store_opt, *force)?;
                progress.done("done.");
                progress.end();
                println!("wrote config {}", store.root.join("config.json").display());
                println!("target      {}", store.config.target.display());
            } else {
                if *force {
                    bail!("--force applies only with --config-only");
                }
                progress.begin("Initializing store");
                let store = Store::init(target_path, store_opt)?;
                progress.done("done.");
                progress.end();
                println!("initialized {}", store.root.display());
                println!("target      {}", store.config.target.display());
                println!(
                    "exclude_dir_names {} names (edit config.json to change)",
                    store.config.settings.exclude_dir_names.len()
                );
            }
        }
        Command::Snap {
            message,
            rehash,
            compress,
        } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let _lock = store.lock()?;
            progress.done("ready.");
            let mut pace = prepare_pace(&cli)?;
            let options = snapshot::SnapshotOptions::from_flags(*rehash, *compress);
            let started = Instant::now();
            let outcome = snapshot::create_with_pace_locked(
                &store,
                message.clone(),
                options,
                pace.as_mut(),
                &mut progress,
            )?;
            let elapsed = format_elapsed(started.elapsed());
            progress.end();
            let files = outcome
                .manifest
                .entries
                .iter()
                .filter(|entry| entry.kind == model::EntryKind::File)
                .count();
            let new_bytes = select::format_bytes(outcome.ingested_bytes);
            let mut flags = Vec::new();
            if *rehash {
                flags.push("rehash");
            }
            if *compress {
                flags.push("compress");
            }
            if flags.is_empty() {
                println!(
                    "created {} ({} files, reused {}, +{new_bytes} new, skipped {} dirs) in {elapsed}",
                    outcome.manifest.id, files, outcome.reused_files, outcome.skipped_dirs
                );
            } else {
                println!(
                    "created {} ({} files, reused {}, +{new_bytes} new, {}, skipped {} dirs) in {elapsed}",
                    outcome.manifest.id,
                    files,
                    outcome.reused_files,
                    flags.join("+"),
                    outcome.skipped_dirs
                );
            }
        }
        Command::Log { max_count } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            let rows = inspect::list_log_rows(&store, &mut progress, *max_count)?;
            progress.end();
            for row in rows {
                println!(
                    "{}  {}  {} entries  {}",
                    short_snapshot_id(&row.id),
                    row.created_at,
                    row.entry_count,
                    row.message.as_deref().unwrap_or("")
                );
            }
        }
        Command::Tree { id, path, depth } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            let outcome = browse::tree(&store, id, path.as_deref(), *depth, &mut progress)?;
            progress.end();
            for line in &outcome.lines {
                println!("{line}");
            }
            println!(
                "tree: {} entries ({} files, {}){}",
                outcome.summary.entry_count,
                outcome.summary.file_count,
                select::format_bytes(outcome.summary.file_bytes),
                outcome
                    .filter
                    .as_ref()
                    .map(|value| format!(" under {value}"))
                    .unwrap_or_default()
            );
        }
        Command::Find { id, pattern } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            let outcome = browse::find(&store, id, pattern, &mut progress)?;
            progress.end();
            for line in &outcome.lines {
                println!("{line}");
            }
            println!(
                "find: {} matches ({} files, {}) for '{}'",
                outcome.summary.entry_count,
                outcome.summary.file_count,
                select::format_bytes(outcome.summary.file_bytes),
                outcome.pattern
            );
        }
        Command::Restore {
            id,
            destination,
            path,
            dry_run,
        } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            let mut pace = prepare_pace(&cli)?;
            let started = Instant::now();
            let outcome = restore::restore_with_pace(
                &store,
                id,
                destination,
                restore::RestoreOptions {
                    path: path.clone(),
                    dry_run: *dry_run,
                },
                pace.as_mut(),
                &mut progress,
            )?;
            let elapsed = format_elapsed(started.elapsed());
            progress.end();
            let scope = outcome
                .filter
                .as_ref()
                .map(|value| format!(" under {value}"))
                .unwrap_or_default();
            let size = select::format_bytes(outcome.summary.file_bytes);
            if outcome.dry_run {
                println!(
                    "dry-run: {} entries ({} files, {}){} -> {} in {elapsed}",
                    outcome.summary.entry_count,
                    outcome.summary.file_count,
                    size,
                    scope,
                    outcome.destination.display()
                );
            } else {
                println!(
                    "restored {} entries ({} files, {}){} to {} in {elapsed}",
                    outcome.summary.entry_count,
                    outcome.summary.file_count,
                    size,
                    scope,
                    outcome.destination.display()
                );
            }
        }
        Command::Verify => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            let mut pace = prepare_pace(&cli)?;
            let started = Instant::now();
            let outcome =
                inspect::verify_with_pace(&store, &mut progress, pace.as_mut())?;
            let elapsed = format_elapsed(started.elapsed());
            progress.end();
            if outcome.skipped.is_empty() {
                println!(
                    "verified {} snapshots and {} objects in {elapsed}",
                    outcome.snapshots, outcome.objects
                );
            } else {
                println!(
                    "verified {} snapshots and {} objects (skipped {} unreadable manifest(s)) in {elapsed}",
                    outcome.snapshots,
                    outcome.objects,
                    outcome.skipped.len()
                );
            }
            reject_if_skipped_snapshots(&outcome.skipped)?;
        }
        Command::Care => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let _lock = store.lock()?;
            progress.done("ready.");
            let mut pace = prepare_pace(&cli)?;
            let started = Instant::now();
            let outcome = care::care_with_pace_locked(&store, pace.as_mut(), &mut progress)?;
            let elapsed = format_elapsed(started.elapsed());
            progress.end();
            if outcome.skipped.is_empty() {
                println!(
                    "care complete: {} snapshots, {} objects ({} compressed, {} unchanged) in {elapsed}",
                    outcome.snapshots,
                    outcome.objects,
                    outcome.compact.compressed,
                    outcome.compact.unchanged
                );
            } else {
                println!(
                    "care complete: {} snapshots, {} objects ({} compressed, {} unchanged, skipped {} unreadable manifest(s)) in {elapsed}",
                    outcome.snapshots,
                    outcome.objects,
                    outcome.compact.compressed,
                    outcome.compact.unchanged,
                    outcome.skipped.len()
                );
            }
            reject_if_skipped_snapshots(&outcome.skipped)?;
        }
        Command::Config => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Reading config");
            let tree = resolve_target(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            progress.done("done.");
            progress.end();
            println!("{}", serde_json::to_string_pretty(&store.config)?);
        }
        Command::Install => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Installing snapline");
            let destination = install::install()?;
            progress.done("done.");
            progress.end();
            println!("installed {}", destination.display());
            println!("open a new terminal, then run: snapline --help");
            #[cfg(not(windows))]
            {
                if let Some(parent) = destination.parent() {
                    println!(
                        "if the command is not found, add this directory to PATH: {}",
                        parent.display()
                    );
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{Cli, normalize_log_digit_limits, reject_if_skipped_snapshots};

    // ============================================================================
    // 正規化後の引列で CLI を解釈する。
    // ============================================================================
    fn parse_cli(args: &[&str]) -> Result<Cli, clap::error::Error> {
        let normalized = normalize_log_digit_limits(args.iter().map(|value| value.to_string()));
        Cli::try_parse_from(normalized)
    }

    // ============================================================================
    // log サブコマンドの引数解釈を確認する。
    // ============================================================================
    #[test]
    fn parses_tree_and_log_command() {
        assert!(parse_cli(&["snapline", "--target", "workspace", "log"]).is_ok());
    }

    // ============================================================================
    // log -1 と --max-count を同じ意味として解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_log_newest_limit_forms() {
        let from_digit = parse_cli(&["snapline", "log", "-1"]).unwrap();
        let from_flag = parse_cli(&["snapline", "log", "--max-count", "1"]).unwrap();
        let from_short = parse_cli(&["snapline", "log", "-n", "1"]).unwrap();
        match (from_digit.command, from_flag.command, from_short.command) {
            (
                super::Command::Log {
                    max_count: Some(1),
                },
                super::Command::Log {
                    max_count: Some(1),
                },
                super::Command::Log {
                    max_count: Some(1),
                },
            ) => {}
            other => panic!("unexpected log parse result: {other:?}"),
        }
    }

    // ============================================================================
    // config サブコマンドの引数解釈を確認する。
    // ============================================================================
    #[test]
    fn parses_config_command() {
        assert!(parse_cli(&["snapline", "--target", "workspace", "config"]).is_ok());
    }

    // ============================================================================
    // init が位置引数で対象ツリーを受け取れることを確認する。
    // ============================================================================
    #[test]
    fn parses_init_target() {
        assert!(parse_cli(&["snapline", "init", "workspace"]).is_ok());
        assert!(parse_cli(&["snapline", "init", "--config-only"]).is_ok());
        assert!(parse_cli(&["snapline", "init", "--config-only", "--force"]).is_ok());
    }

    // ============================================================================
    // 外部ストア指定オプションを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_external_store_option() {
        assert!(
            parse_cli(&[
                "snapline",
                "--target",
                "workspace",
                "--store",
                "D:/stores/work",
                "snap"
            ])
            .is_ok()
        );
    }

    // ============================================================================
    // install サブコマンドを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_install_command() {
        assert!(parse_cli(&["snapline", "install"]).is_ok());
    }

    // ============================================================================
    // --background を snap に付けられることを確認する。
    // ============================================================================
    #[test]
    fn parses_background_option_on_snap() {
        assert!(parse_cli(&["snapline", "--background", "snap", "-m", "idle"]).is_ok());
    }

    // ============================================================================
    // snap --rehash / --compress と care を解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_snap_flags_and_care() {
        let cli = parse_cli(&["snapline", "snap", "--rehash", "--compress"]).unwrap();
        match cli.command {
            super::Command::Snap {
                rehash: true,
                compress: true,
                ..
            } => {}
            other => panic!("unexpected snap parse result: {other:?}"),
        }
        assert!(parse_cli(&["snapline", "care"]).is_ok());
        assert!(parse_cli(&["snapline", "snap", "--full"]).is_err());
    }

    // ============================================================================
    // 旧 snapshot / list サブコマンド名は受け付けないことを確認する。
    // ============================================================================
    #[test]
    fn rejects_old_command_names() {
        assert!(parse_cli(&["snapline", "list"]).is_err());
        assert!(parse_cli(&["snapline", "snapshot"]).is_err());
    }

    // ============================================================================
    // tree / find / restore の新オプションを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_browse_and_restore_options() {
        assert!(parse_cli(&["snapline", "tree", "abcd1234", "--path", "docs", "--depth", "2"]).is_ok());
        assert!(parse_cli(&["snapline", "find", "abcd1234", "readme"]).is_ok());
        assert!(parse_cli(&[
            "snapline",
            "restore",
            "abcd1234",
            "D:/out",
            "--path",
            "docs",
            "--dry-run",
        ])
        .is_ok());
    }

    // ============================================================================
    // トップレベル help に全コマンドと全オプションが載ることを確認する。
    // ============================================================================
    #[test]
    fn top_level_help_lists_every_command_and_option() {
        use clap::CommandFactory;

        let mut help = Vec::new();
        Cli::command()
            .write_long_help(&mut help)
            .expect("write help");
        let text = String::from_utf8(help).expect("utf8 help");
        for name in [
            "init", "snap", "log", "tree", "find", "restore", "verify", "care", "config",
            "install",
        ] {
            assert!(
                text.contains(name),
                "help must list command `{name}`:\n{text}"
            );
        }
        for option in [
            "--target",
            "--store",
            "--background",
            "--cpu-busy-percent",
            "--memory-load-percent",
            "--poll-ms",
            "--rehash",
            "--compress",
            "--message",
            "--max-count",
            "--path",
            "--dry-run",
            "--depth",
            "--config-only",
            "--force",
        ] {
            assert!(
                text.contains(option),
                "help must list option `{option}`:\n{text}"
            );
        }
        assert!(text.contains("Commands:"));
        assert!(text.contains("Global options (any command):"));
        assert!(text.contains("Create a history store"));
        assert!(text.contains("Reread every file and recompute content hashes"));
        assert!(text.contains("Try zstd when ingesting"));
        assert!(text.contains("Check that stored manifests"));
        assert!(text.contains("Run verify, then compress"));
        assert!(text.contains("snap, care, restore, verify only"));
        // オプションがどのコマンド配下か分かる形になっていること。
        assert!(text.contains("snap [-m, --message <MESSAGE>] [--rehash] [--compress]"));
        assert!(text.contains("log [-n, --max-count <N>]"));
        let snap_at = text.find("snap [-m,").expect("snap block");
        let log_at = text.find("log [-n,").expect("log block");
        let rehash_at = text.find("--rehash").expect("rehash");
        let max_count_at = text.find("--max-count").expect("max-count");
        assert!(snap_at < rehash_at && rehash_at < log_at);
        assert!(log_at < max_count_at);
        assert!(!text.contains("Everyday"));
        assert!(!text.contains("Occasional"));
        assert!(!text.contains("Command options:"));
        // clap 自動一覧との二重表示が無いこと。
        assert_eq!(text.matches("Commands:").count(), 1);
        assert!(!text.contains("snapline.exe"));
        assert!(!text.contains("Print this message or the help of the given subcommand"));
        assert!(!text.contains("Verify every referenced object in the store"));
    }

    // ============================================================================
    // 引数なし用の簡略表示に全コマンドがあり、よくある使い方が無いことを確認する。
    // ============================================================================
    #[test]
    fn brief_usage_lists_commands_without_common_usage() {
        let text = super::brief_usage_text();
        for name in [
            "init", "snap", "log", "tree", "find", "restore", "verify", "care", "config",
            "install",
        ] {
            assert!(text.contains(name), "brief usage missing `{name}`");
        }
        assert!(text.contains("snapline --help"));
        assert!(!text.contains("Everyday"));
        assert!(!text.contains("--rehash"));
    }

    // ============================================================================
    // コマンド名の誤りや、コマンドに属さないオプションを拒否することを確認する。
    // ============================================================================
    #[test]
    fn rejects_unknown_commands_and_mismatched_options() {
        assert!(parse_cli(&["snapline", "snapshpt"]).is_err());
        assert!(parse_cli(&["snapline", "verfy"]).is_err());
        assert!(parse_cli(&["snapline", "log", "--rehash"]).is_err());
        assert!(parse_cli(&["snapline", "verify", "--compress"]).is_err());
        assert!(parse_cli(&["snapline", "care", "--message", "x"]).is_err());
        assert!(parse_cli(&["snapline", "snap", "--max-count", "1"]).is_err());
        assert!(parse_cli(&["snapline", "restore"]).is_err());
        assert!(parse_cli(&["snapline", "restore", "only-one-arg"]).is_err());
        assert!(parse_cli(&["snapline", "snap", "--not-a-real-flag"]).is_err());
        assert!(parse_cli(&["snapline", "log", "--max-count", "0"]).is_err());
    }

    // ============================================================================
    // --background を使えないコマンドでは実行前検査が失敗することを確認する。
    // ============================================================================
    #[test]
    fn rejects_background_on_disallowed_commands() {
        for args in [
            &["snapline", "--background", "log"][..],
            &["snapline", "--background", "tree", "abcd"][..],
            &["snapline", "--background", "find", "abcd", "x"][..],
            &["snapline", "--background", "init"][..],
            &["snapline", "--background", "config"][..],
            &["snapline", "--background", "install"][..],
        ] {
            let cli = parse_cli(args).unwrap_or_else(|error| {
                panic!("parse should succeed for {args:?}: {error}")
            });
            let error = super::ensure_background_allowed(&cli)
                .expect_err(&format!("background must be rejected for {args:?}"));
            assert!(
                error.to_string().contains("--background"),
                "unexpected error for {args:?}: {error}"
            );
        }

        for args in [
            &["snapline", "--background", "snap"][..],
            &["snapline", "--background", "care"][..],
            &["snapline", "--background", "verify"][..],
            &[
                "snapline",
                "--background",
                "restore",
                "abcd",
                "D:/out",
            ][..],
        ] {
            let cli = parse_cli(args).unwrap_or_else(|error| {
                panic!("parse should succeed for {args:?}: {error}")
            });
            super::ensure_background_allowed(&cli)
                .unwrap_or_else(|error| panic!("background must be allowed for {args:?}: {error}"));
        }
    }

    // ============================================================================
    // 壊れたスナップをスキップした結果は成功扱いせず拒否することを確認する。
    // ============================================================================
    #[test]
    fn reject_if_skipped_snapshots_fails_when_any_broken() {
        let error = reject_if_skipped_snapshots(&["aaaa-broken".into()])
            .expect_err("skipped snapshots must fail the command");
        assert!(error.to_string().contains("unreadable snapshot manifest"));
        assert!(error.to_string().contains("aaaa-broken"));
    }

    // ============================================================================
    // スキップが無いときは拒否しないことを確認する。
    // ============================================================================
    #[test]
    fn reject_if_skipped_snapshots_ok_when_empty() {
        reject_if_skipped_snapshots(&[]).expect("empty skipped must succeed");
    }
}
