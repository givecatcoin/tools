//! ============================================================================
//! 実行ファイルをユーザー向けの固定場所へ置き、PATH から呼べるようにする。
//! ============================================================================

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

#[cfg(windows)]
const EXE_NAME: &str = "snapline.exe";

#[cfg(not(windows))]
const EXE_NAME: &str = "snapline";

// ============================================================================
// 現在の実行ファイルをユーザー用ディレクトリへコピーし、PATH に登録する。
// 以後はフルパスを書かず `snapline snapshot` のように呼べる。
// ============================================================================
pub fn install() -> Result<PathBuf> {
    let source = env::current_exe().context("failed to locate current executable")?;
    let destination_dir = install_dir()?;
    fs::create_dir_all(&destination_dir)
        .with_context(|| format!("failed to create {}", destination_dir.display()))?;

    let destination = destination_dir.join(EXE_NAME);
    fs::copy(&source, &destination).with_context(|| {
        format!(
            "failed to copy {} -> {}",
            source.display(),
            destination.display()
        )
    })?;

    ensure_dir_on_user_path(&destination_dir)?;
    Ok(destination)
}

// ============================================================================
// インストール先ディレクトリを決める。
// Windows: %LOCALAPPDATA%\Snapline\bin
// その他:  ~/.local/bin
// ============================================================================
fn install_dir() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let local = env::var_os("LOCALAPPDATA")
            .context("LOCALAPPDATA is not set; cannot choose install location")?;
        Ok(PathBuf::from(local).join("Snapline").join("bin"))
    }

    #[cfg(not(windows))]
    {
        let home =
            env::var_os("HOME").context("HOME is not set; cannot choose install location")?;
        Ok(PathBuf::from(home).join(".local").join("bin"))
    }
}

// ============================================================================
// ユーザー PATH にディレクトリが無ければ追加する。
// 既に含まれている場合は何もしない。
// ============================================================================
fn ensure_dir_on_user_path(dir: &Path) -> Result<()> {
    let dir_text = dir
        .to_str()
        .context("install path contains non-UTF-8 characters")?;

    #[cfg(windows)]
    {
        // User 環境変数 Path を読み、未登録なら追記する。
        // 新しいターミナルから有効になる（現在のセッションには自動反映しない）。
        let script = format!(
            r#"
$dir = '{dir}'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if ($null -eq $userPath) {{ $userPath = '' }}
$parts = $userPath -split ';' | Where-Object {{ $_ -ne '' }}
$already = $parts | Where-Object {{ $_.TrimEnd('\') -ieq $dir.TrimEnd('\') }}
if (-not $already) {{
    if ($userPath -eq '') {{
        [Environment]::SetEnvironmentVariable('Path', $dir, 'User')
    }} else {{
        [Environment]::SetEnvironmentVariable('Path', ($userPath.TrimEnd(';') + ';' + $dir), 'User')
    }}
    Write-Output 'added'
}} else {{
    Write-Output 'present'
}}
"#,
            dir = dir_text.replace('\'', "''")
        );

        let output = Command::new("powershell")
            .args(["-NoProfile", "-Command", &script])
            .output()
            .context("failed to update user PATH via PowerShell")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("failed to update user PATH: {stderr}");
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        let _ = dir_text;
        // PATH 追記はシェル設定に依存するため、呼び出し側で案内する。
        Ok(())
    }
}
