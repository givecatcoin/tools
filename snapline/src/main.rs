//! ============================================================================
//! CLI 入口。ユーザー操作を各モジュールへ橋渡しする。
//! ============================================================================

mod background;
mod inspect;
mod install;
mod model;
mod object;
mod pace;
mod restore;
mod settings;
mod snaplinenore;
mod snapshot;
mod store;

use std::{
    io::{self, IsTerminal},
    path::PathBuf,
    time::Duration,
};

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::{
    background::{BackgroundLimits, activate},
    store::Store,
};

// ============================================================================
// 共通オプションとサブコマンドをまとめた CLI 定義。
// `--tree` は履歴対象ツリー。省略時はカレントディレクトリから親方向へ
// `.snapline` を探し、最も近い場所を対象ツリーとする。
// `--store` は任意のストア配置先。
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
    /// List snapshots from oldest to newest.
    List,
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
    /// Run heavy operations with low priority and resource-aware pacing.
    Background {
        #[command(flatten)]
        limits: BackgroundLimitsArgs,
        #[command(subcommand)]
        command: BackgroundCommand,
    },
}

// ============================================================================
// バックグラウンド実行の資源しきい値。通常コマンドとは別経路でのみ使う。
// ============================================================================
#[derive(Debug, Clone, clap::Args)]
struct BackgroundLimitsArgs {
    /// Wait while total CPU usage exceeds this percent (1..=100).
    #[arg(long, default_value_t = background::DEFAULT_CPU_BUSY_PERCENT)]
    cpu_busy_percent: u8,

    /// Wait while physical memory load exceeds this percent (1..=100).
    #[arg(long, default_value_t = background::DEFAULT_MEMORY_LOAD_PERCENT)]
    memory_load_percent: u8,

    /// Milliseconds between resource checks while waiting.
    #[arg(long, default_value = "200")]
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
// バックグラウンド経路で実行できる操作。init / list / config は対象外。
// ============================================================================
#[derive(Debug, Subcommand)]
enum BackgroundCommand {
    /// Capture the current target directory with resource-aware pacing.
    Snapshot {
        #[arg(short, long)]
        message: Option<String>,
    },
    /// Restore a snapshot with resource-aware pacing.
    Restore {
        #[arg(value_name = "SNAPSHOT_ID")]
        id: String,
        #[arg(value_name = "DESTINATION")]
        destination: PathBuf,
    },
    /// Verify the store with resource-aware pacing.
    Verify,
}

// ============================================================================
// 既存ストアを使うコマンドの対象ツリーを決める。
// 明示された `--tree` を優先し、省略時だけカレントから親方向へ探索する。
// ============================================================================
fn resolve_tree(cli: &Cli) -> Result<PathBuf> {
    match &cli.tree {
        Some(tree) => Ok(tree.clone()),
        None => store::discover_tree_root(&std::env::current_dir()?),
    }
}

// ============================================================================
// 一覧表示用の短縮 ID を返す。
// 現行 ID の末尾 UUID 部分を使うため、同日に作った履歴でも短く識別しやすい。
// ============================================================================
fn short_snapshot_id(id: &str) -> &str {
    let suffix = id.rsplit_once('-').map_or(id, |(_, suffix)| suffix);
    &suffix[..suffix.len().min(12)]
}

// ============================================================================
// CLI を解釈し、対応する処理へ振り分ける。
// ============================================================================
fn main() -> Result<()> {
    let cli = Cli::parse();
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
            let outcome = snapshot::create(&store, message.clone())?;
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
        Command::List => {
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
            let count = restore::restore(&store, id, destination)?;
            println!("restored {count} entries to {}", destination.display());
        }
        Command::Verify => {
            let tree = resolve_tree(&cli)?;
            let store = Store::open(&tree, store_opt)?;
            let progress: Box<dyn io::Write> = if io::stderr().is_terminal() {
                Box::new(io::stderr())
            } else {
                Box::new(io::sink())
            };
            let (snapshots, objects) = inspect::verify(&store, progress)?;
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
        Command::Background { limits, command } => {
            run_background(&cli, store_opt, limits.clone(), command)?;
        }
    }

    Ok(())
}

// ============================================================================
// バックグラウンド実行。本体ロジックは既存関数の _with_pace 版を使い、
// 低優先度と資源監視だけを background モジュールに閉じ込める。
// ============================================================================
fn run_background(
    cli: &Cli,
    store_opt: Option<&std::path::Path>,
    limits_args: BackgroundLimitsArgs,
    command: &BackgroundCommand,
) -> Result<()> {
    let mut pace = activate(limits_args.into_limits())?;
    let tree = resolve_tree(cli)?;
    let store = Store::open(&tree, store_opt)?;

    match command {
        BackgroundCommand::Snapshot { message } => {
            let outcome = snapshot::create_with_pace(&store, message.clone(), &mut pace)?;
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
        BackgroundCommand::Restore { id, destination } => {
            let count = restore::restore_with_pace(&store, id, destination, &mut pace)?;
            println!("restored {count} entries to {}", destination.display());
        }
        BackgroundCommand::Verify => {
            let progress: Box<dyn io::Write> = if io::stderr().is_terminal() {
                Box::new(io::stderr())
            } else {
                Box::new(io::sink())
            };
            let (snapshots, objects) = inspect::verify_with_pace(&store, progress, &mut pace)?;
            println!("verified {snapshots} snapshots and {objects} objects");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::Cli;

    // ============================================================================
    // list サブコマンドの引数解釈を確認する。
    // ============================================================================
    #[test]
    fn parses_tree_and_list_command() {
        assert!(Cli::try_parse_from(["snapline", "--tree", "workspace", "list"]).is_ok());
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
    // background サブコマンドを解釈できることを確認する。
    // ============================================================================
    #[test]
    fn parses_background_snapshot_command() {
        assert!(Cli::try_parse_from(["snapline", "background", "snapshot", "-m", "idle"]).is_ok());
    }
}
