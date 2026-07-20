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
mod snaplinenore;
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
    Log,
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
// `--background` が使える操作かどうかを検査する。
// 使えない操作に付けた場合はエラー（無視して続行しない）。
// ============================================================================
fn ensure_background_allowed(cli: &Cli) -> Result<()> {
    if !cli.background {
        return Ok(());
    }
    match cli.command {
        Command::Snapshot { .. } | Command::Restore { .. } | Command::Verify => Ok(()),
        Command::Init { .. } | Command::Log | Command::Config | Command::Install => {
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
    let cli = Cli::parse();
    ensure_background_allowed(&cli)?;
    let store_opt = cli.store.as_deref();

    match &cli.command {
        Command::Init { target } => {
            let current = std::env::current_dir()?;
            let target = target
                .as_deref()
                .or(cli.tree.as_deref())
                .unwrap_or(current.as_path());
            let store = Store::init(target, store_opt)?;
            println!("initialized {}", store.root.display());
            println!("target      {}", store.config.target.display());
            println!(
                "exclude_dir_names {} names (edit config.json to change)",
                store.config.settings.exclude_dir_names.len()
            );
            println!("protect_git {}", store.config.settings.protect_git);
        }
        Command::Snapshot { message } => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_if_tty();
            let outcome =
                snapshot::create_with_pace(&store, message.clone(), pace.as_mut(), &mut progress)?;
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
        Command::Log => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            for manifest in inspect::list(&store)? {
                println!(
                    "{}  {}  {} entries  {}",
                    short_snapshot_id(&manifest.id),
                    manifest.created_at,
                    manifest.entries.len(),
                    manifest.message.as_deref().unwrap_or("")
                );
            }
        }
        Command::Restore { id, destination } => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_if_tty();
            let count = restore::restore_with_pace(
                &store,
                id,
                destination,
                pace.as_mut(),
                &mut progress,
            )?;
            println!("restored {count} entries to {}", destination.display());
        }
        Command::Verify => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let mut pace = prepare_pace(&cli)?;
            let mut progress = Progress::stderr_if_tty();
            let (snapshots, objects) =
                inspect::verify_with_pace(&store, &mut progress, pace.as_mut())?;
            println!("verified {snapshots} snapshots and {objects} objects");
        }
        Command::Config => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            println!("{}", serde_json::to_string_pretty(&store.config)?);
        }
        Command::Install => {
            let destination = install::install()?;
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

    use super::Cli;

    // ============================================================================
    // log サブコマンドの引数解釈を確認する。
    // ============================================================================
    #[test]
    fn parses_tree_and_log_command() {
        assert!(Cli::try_parse_from(["snapline", "--tree", "workspace", "log"]).is_ok());
    }

    // ============================================================================
    // config サブコマンドの引数解釈を確認する。
    // ============================================================================
    #[test]
    fn parses_config_command() {
        assert!(Cli::try_parse_from(["snapline", "--tree", "workspace", "config"]).is_ok());
    }

    // ============================================================================
    // init が位置引数で対象ツリーを受け取れることを確認する。
    // ============================================================================
    #[test]
    fn parses_init_target() {
        assert!(Cli::try_parse_from(["snapline", "init", "workspace"]).is_ok());
    }

    // ============================================================================
    // 外部ストア指定オプションを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_external_store_option() {
        assert!(
            Cli::try_parse_from([
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
        assert!(Cli::try_parse_from(["snapline", "install"]).is_ok());
    }

    // ============================================================================
    // --background を通常コマンドのオプションとして解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_background_option_on_snapshot() {
        assert!(
            Cli::try_parse_from(["snapline", "--background", "snapshot", "-m", "idle"]).is_ok()
        );
    }

    // ============================================================================
    // 旧 list サブコマンド名は受け付けないことを確認する。
    // ============================================================================
    #[test]
    fn rejects_old_list_command_name() {
        assert!(Cli::try_parse_from(["snapline", "list"]).is_err());
    }
}
