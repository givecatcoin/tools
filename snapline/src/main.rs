//! ============================================================================
//! CLI 入口。ユーザー操作を各モジュールへ橋渡しする。
//! ============================================================================

mod background;
mod inspect;
mod install;
mod model;
mod object;
mod pace;
mod progress;
mod restore;
mod settings;
mod snaplineignore;
mod snapshot;
mod store;

use std::{
    path::PathBuf,
    time::Duration,
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};

use crate::{
    background::{BackgroundLimits, BackgroundPace, activate},
    pace::{IdlePace, IoPace},
    progress::Progress,
    store::Store,
};

// ============================================================================
// 共通オプションとサブコマンドをまとめた CLI 定義。
// `--tree` は履歴対象ツリー。省略時はカレントから親方向へ `.snapline` を探す。
// `--store` は任意のストア配置先。
// `--background` は重い操作を低優先度・資源監視付きで進める。
// ============================================================================
#[derive(Debug, Parser)]
#[command(
    version,
    about = "Safe, content-addressed directory snapshots without Git semantics"
)]
struct Cli {
    /// Target tree root directory. Discovered from the current directory when omitted.
    #[arg(long, global = true, env = "SNAPLINE_TREE")]
    tree: Option<PathBuf>,

    /// Optional store location. Defaults to <tree>/.snapline.
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
    },
    /// Capture the current target directory.
    Snapshot {
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Show snapshots from oldest to newest.
    Log {
        /// Show only the newest N snapshots (e.g. `-1` for the latest only).
        #[arg(short = 'n', long = "max-count", value_name = "N", value_parser = parse_max_count)]
        max_count: Option<usize>,
    },
    /// Restore a snapshot into a new or empty directory.
    Restore {
        #[arg(value_name = "SNAPSHOT_ID")]
        id: String,
        #[arg(value_name = "DESTINATION")]
        destination: PathBuf,
    },
    /// Verify every referenced object in the store.
    Verify,
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
fn resolve_tree(cli: &Cli) -> Result<PathBuf> {
    match &cli.tree {
        Some(tree) => Ok(tree.clone()),
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
        Command::Snapshot { .. } | Command::Restore { .. } | Command::Verify => Ok(()),
        Command::Init { .. } | Command::Log { .. } | Command::Config | Command::Install => {
            bail!("--background applies only to snapshot, restore, and verify")
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
// CLI を解釈し、対応する処理へ振り分ける。
// ============================================================================
fn main() -> Result<()> {
    let cli = Cli::parse_from(normalize_log_digit_limits(std::env::args()));
    ensure_background_allowed(&cli)?;
    let store_opt = cli.store.as_deref();

    match &cli.command {
        Command::Init { target } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Initializing store");
            let current = std::env::current_dir()?;
            let target = target
                .as_deref()
                .or(cli.tree.as_deref())
                .unwrap_or(current.as_path());
            let store = Store::init(target, store_opt)?;
            progress.done("done.");
            progress.end();
            println!("initialized {}", store.root.display());
            println!("target      {}", store.config.target.display());
            println!(
                "exclude_dir_names {} names (edit config.json to change)",
                store.config.settings.exclude_dir_names.len()
            );
        }
        Command::Snapshot { message } => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_for_cli();
            let outcome =
                snapshot::create_with_pace(&store, message.clone(), pace.as_mut(), &mut progress)?;
            progress.end();
            let files = outcome
                .manifest
                .entries
                .iter()
                .filter(|entry| entry.kind == model::EntryKind::File)
                .count();
            println!(
                "created {} ({} files, skipped {} dirs)",
                outcome.manifest.id, files, outcome.skipped_dirs
            );
        }
        Command::Log { max_count } => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Opening store");
            let tree = resolve_tree(&cli)?;
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
        Command::Restore { id, destination } => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_for_cli();
            let count = restore::restore_with_pace(
                &store,
                id,
                destination,
                pace.as_mut(),
                &mut progress,
            )?;
            progress.end();
            println!("restored {count} entries to {}", destination.display());
        }
        Command::Verify => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_for_cli();
            let (snapshots, objects) =
                inspect::verify_with_pace(&store, &mut progress, pace.as_mut())?;
            progress.end();
            println!("verified {snapshots} snapshots and {objects} objects");
        }
        Command::Config => {
            let mut progress = Progress::stderr_for_cli();
            progress.begin("Reading config");
            let tree = resolve_tree(&cli)?;
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

    use super::{Cli, normalize_log_digit_limits};

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
        assert!(parse_cli(&["snapline", "--tree", "workspace", "log"]).is_ok());
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
        assert!(parse_cli(&["snapline", "--tree", "workspace", "config"]).is_ok());
    }

    // ============================================================================
    // init が位置引数で対象ツリーを受け取れることを確認する。
    // ============================================================================
    #[test]
    fn parses_init_target() {
        assert!(parse_cli(&["snapline", "init", "workspace"]).is_ok());
    }

    // ============================================================================
    // 外部ストア指定オプションを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_external_store_option() {
        assert!(
            parse_cli(&[
                "snapline",
                "--tree",
                "workspace",
                "--store",
                "D:/stores/work",
                "snapshot"
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
    // --background を通常コマンドのオプションとして解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_background_option_on_snapshot() {
        assert!(
            parse_cli(&["snapline", "--background", "snapshot", "-m", "idle"]).is_ok()
        );
    }

    // ============================================================================
    // 旧 list サブコマンド名は受け付けないことを確認する。
    // ============================================================================
    #[test]
    fn rejects_old_list_command_name() {
        assert!(parse_cli(&["snapline", "list"]).is_err());
    }
}
