//! ============================================================================
//! バックグラウンド実行専用の資源監視と待機。
//!
//! 既存の snapshot / object / restore 本体とは独立したモジュール。
//! 通常コマンド経路からは呼ばない。
//!
//! 方針:
//! ゲームやブラウジングを邪魔しないよう、低優先度と資源しきい値で待機する。
//! 監視や待機に失敗したらエラーにする（黙って全力実行へフォールバックしない）。
//! ファイルを飛ばす・検証を緩めるといった曖昧な妥協はしない。

use std::{thread, time::Duration};

use anyhow::{Context, Result, bail};

use crate::pace::IoPace;

/// CPU 使用率（%）がこれ以上なら待機する既定値。
pub const DEFAULT_CPU_BUSY_PERCENT: u8 = 70;

/// 物理メモリ使用率（%）がこれ以上なら待機する既定値。
pub const DEFAULT_MEMORY_LOAD_PERCENT: u8 = 90;

/// 資源再確認の間隔。
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(200);

/// このバイト数処理するごとに資源を再確認する。
const CHUNK_CHECK_BYTES: u64 = 4 * 1024 * 1024;

// ============================================================================
// バックグラウンド実行のしきい値。明示指定以外は既定値。
// ============================================================================
#[derive(Debug, Clone)]
pub struct BackgroundLimits {
    pub cpu_busy_percent: u8,
    pub memory_load_percent: u8,
    pub poll_interval: Duration,
}

impl Default for BackgroundLimits {
    fn default() -> Self {
        Self {
            cpu_busy_percent: DEFAULT_CPU_BUSY_PERCENT,
            memory_load_percent: DEFAULT_MEMORY_LOAD_PERCENT,
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

impl BackgroundLimits {
    // ===========================================================================
    // しきい値が解釈可能か検査する。曖昧な値は拒否する。
    // ===========================================================================
    pub fn validate(&self) -> Result<()> {
        if self.cpu_busy_percent == 0 || self.cpu_busy_percent > 100 {
            bail!(
                "cpu busy percent must be 1..=100, got {}",
                self.cpu_busy_percent
            );
        }
        if self.memory_load_percent == 0 || self.memory_load_percent > 100 {
            bail!(
                "memory load percent must be 1..=100, got {}",
                self.memory_load_percent
            );
        }
        if self.poll_interval.is_zero() {
            bail!("poll interval must be greater than zero");
        }
        Ok(())
    }
}

// ============================================================================
// 低優先度を有効化し、資源監視付きペースを返す。
// 優先度変更に失敗した場合はエラー（通常優先度のまま進めない）。
// ============================================================================
pub fn activate(limits: BackgroundLimits) -> Result<BackgroundPace> {
    limits.validate()?;
    lower_process_priority().context("failed to enter background priority mode")?;
    let sampler = ResourceSampler::new().context("failed to start resource sampler")?;
    Ok(BackgroundPace::new(limits, sampler))
}

// ============================================================================
// 資源が空くまで待機するペース制御。
// ============================================================================
pub struct BackgroundPace {
    limits: BackgroundLimits,
    sampler: ResourceSampler,
    bytes_since_check: u64,
}

impl BackgroundPace {
    fn new(limits: BackgroundLimits, sampler: ResourceSampler) -> Self {
        Self {
            limits,
            sampler,
            bytes_since_check: 0,
        }
    }

    // ===========================================================================
    // CPU / メモリがしきい値未満になるまで待つ。
    // 監視失敗はエラー。タイムアウトによる「だいたい成功」はしない。
    // ===========================================================================
    fn wait_until_clear(&mut self) -> Result<()> {
        loop {
            let sample = self
                .sampler
                .sample()
                .context("failed to sample system resources")?;
            let cpu_busy = sample.cpu_busy_percent > self.limits.cpu_busy_percent;
            let memory_busy = sample.memory_load_percent > self.limits.memory_load_percent;
            if !cpu_busy && !memory_busy {
                return Ok(());
            }
            thread::sleep(self.limits.poll_interval);
        }
    }
}

impl IoPace for BackgroundPace {
    fn before_entry(&mut self) -> Result<()> {
        self.wait_until_clear()
    }

    fn after_chunk(&mut self, bytes: usize) -> Result<()> {
        self.bytes_since_check = self.bytes_since_check.saturating_add(bytes as u64);
        if self.bytes_since_check >= CHUNK_CHECK_BYTES {
            self.bytes_since_check = 0;
            self.wait_until_clear()?;
        }
        Ok(())
    }
}

// ============================================================================
// 1 回の観測結果。
// ============================================================================
#[derive(Debug, Clone, Copy)]
struct ResourceSample {
    cpu_busy_percent: u8,
    memory_load_percent: u8,
}

// ============================================================================
// OS から CPU / メモリを読む。プラットフォームごとに実装を分ける。
// ============================================================================
struct ResourceSampler {
    #[cfg(windows)]
    previous: Option<WindowsCpuTimes>,
    #[cfg(not(windows))]
    previous_cpu: Option<(u64, u64)>,
}

impl ResourceSampler {
    fn new() -> Result<Self> {
        let mut sampler = Self {
            #[cfg(windows)]
            previous: None,
            #[cfg(not(windows))]
            previous_cpu: None,
        };
        // 初回は差分が取れないので、短い間隔で 2 度読んで基準を作る。
        let _ = sampler.sample()?;
        thread::sleep(Duration::from_millis(50));
        let _ = sampler.sample()?;
        Ok(sampler)
    }

    fn sample(&mut self) -> Result<ResourceSample> {
        let memory_load_percent = memory_load_percent()?;
        let cpu_busy_percent = self.cpu_busy_percent()?;
        Ok(ResourceSample {
            cpu_busy_percent,
            memory_load_percent,
        })
    }

    #[cfg(windows)]
    fn cpu_busy_percent(&mut self) -> Result<u8> {
        let current = WindowsCpuTimes::read()?;
        let Some(previous) = self.previous.replace(current) else {
            return Ok(0);
        };
        current.busy_percent_since(&previous)
    }

    #[cfg(not(windows))]
    fn cpu_busy_percent(&mut self) -> Result<u8> {
        // Linux: /proc/stat の集計。差分が取れない初回は 0。
        let text =
            std::fs::read_to_string("/proc/stat").context("failed to read /proc/stat for CPU")?;
        let line = text.lines().next().context("/proc/stat is empty")?;
        let mut parts = line.split_whitespace();
        if parts.next() != Some("cpu") {
            bail!("unexpected /proc/stat format");
        }
        let values: Vec<u64> = parts.filter_map(|value| value.parse().ok()).collect();
        if values.len() < 4 {
            bail!("unexpected /proc/stat cpu counters");
        }
        let idle = values[3].saturating_add(values.get(4).copied().unwrap_or(0));
        let total: u64 = values.iter().sum();
        let Some((prev_idle, prev_total)) = self.previous_cpu.replace((idle, total)) else {
            return Ok(0);
        };
        let idle_delta = idle.saturating_sub(prev_idle);
        let total_delta = total.saturating_sub(prev_total);
        if total_delta == 0 {
            return Ok(0);
        }
        let busy = total_delta.saturating_sub(idle_delta);
        Ok(((busy.saturating_mul(100)) / total_delta).min(100) as u8)
    }
}

#[cfg(windows)]
#[derive(Clone, Copy)]
struct WindowsCpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

#[cfg(windows)]
impl WindowsCpuTimes {
    fn read() -> Result<Self> {
        unsafe {
            let mut idle = zero_filetime();
            let mut kernel = zero_filetime();
            let mut user = zero_filetime();
            if GetSystemTimes(&mut idle, &mut kernel, &mut user) == 0 {
                bail!("GetSystemTimes failed with error {}", GetLastError());
            }
            Ok(Self {
                idle: filetime_to_u64(idle),
                kernel: filetime_to_u64(kernel),
                user: filetime_to_u64(user),
            })
        }
    }

    fn busy_percent_since(&self, previous: &Self) -> Result<u8> {
        // Windows では kernel 時間に idle が含まれる。
        let idle_delta = self.idle.saturating_sub(previous.idle);
        let kernel_delta = self.kernel.saturating_sub(previous.kernel);
        let user_delta = self.user.saturating_sub(previous.user);
        let total_delta = kernel_delta.saturating_add(user_delta);
        if total_delta == 0 {
            return Ok(0);
        }
        if idle_delta > total_delta {
            bail!("invalid CPU counters: idle delta exceeds total delta");
        }
        let busy_delta = total_delta - idle_delta;
        Ok(((busy_delta.saturating_mul(100)) / total_delta).min(100) as u8)
    }
}

// ============================================================================
// プロセスを低優先度＋バックグラウンド I/O 優先度へ移す。
// ============================================================================
fn lower_process_priority() -> Result<()> {
    #[cfg(windows)]
    {
        unsafe {
            let process = GetCurrentProcess();
            // バックグラウンドモードは CPU・ディスク I/O の優先度をまとめて下げる。
            // 失敗時に通常優先度のまま続行することはしない。
            if SetPriorityClass(process, PROCESS_MODE_BACKGROUND_BEGIN) == 0 {
                bail!(
                    "PROCESS_MODE_BACKGROUND_BEGIN failed with error {}",
                    GetLastError()
                );
            }
        }
        Ok(())
    }

    #[cfg(not(windows))]
    {
        // nice を上げる（優先度を下げる）。失敗はエラー。
        let result = unsafe { libc_setpriority() };
        if result != 0 {
            bail!("failed to lower process niceness");
        }
        Ok(())
    }
}

#[cfg(not(windows))]
fn libc_setpriority() -> i32 {
    extern "C" {
        fn setpriority(which: i32, who: u32, prio: i32) -> i32;
    }
    const PRIO_PROCESS: i32 = 0;
    unsafe { setpriority(PRIO_PROCESS, 0, 10) }
}

fn memory_load_percent() -> Result<u8> {
    #[cfg(windows)]
    {
        unsafe {
            let mut status = MemoryStatusEx {
                length: std::mem::size_of::<MemoryStatusEx>() as u32,
                memory_load: 0,
                total_phys: 0,
                avail_phys: 0,
                total_page_file: 0,
                avail_page_file: 0,
                total_virtual: 0,
                avail_virtual: 0,
                avail_extended_virtual: 0,
            };
            if GlobalMemoryStatusEx(&mut status) == 0 {
                bail!("GlobalMemoryStatusEx failed with error {}", GetLastError());
            }
            Ok(status.memory_load.min(100) as u8)
        }
    }

    #[cfg(not(windows))]
    {
        let text =
            std::fs::read_to_string("/proc/meminfo").context("failed to read /proc/meminfo")?;
        let mut total_kb = None;
        let mut available_kb = None;
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("MemTotal:") {
                total_kb = Some(parse_meminfo_kb(value)?);
            } else if let Some(value) = line.strip_prefix("MemAvailable:") {
                available_kb = Some(parse_meminfo_kb(value)?);
            }
        }
        let total = total_kb.context("MemTotal missing in /proc/meminfo")?;
        let available = available_kb.context("MemAvailable missing in /proc/meminfo")?;
        if total == 0 {
            bail!("MemTotal is zero");
        }
        let used = total.saturating_sub(available);
        Ok(((used.saturating_mul(100)) / total).min(100) as u8)
    }
}

#[cfg(not(windows))]
fn parse_meminfo_kb(value: &str) -> Result<u64> {
    let number = value
        .split_whitespace()
        .next()
        .context("invalid /proc/meminfo value")?;
    number
        .parse()
        .with_context(|| format!("invalid /proc/meminfo number: {number}"))
}

#[cfg(windows)]
#[repr(C)]
struct FileTime {
    dw_low_date_time: u32,
    dw_high_date_time: u32,
}

#[cfg(windows)]
fn zero_filetime() -> FileTime {
    FileTime {
        dw_low_date_time: 0,
        dw_high_date_time: 0,
    }
}

#[cfg(windows)]
fn filetime_to_u64(value: FileTime) -> u64 {
    ((value.dw_high_date_time as u64) << 32) | value.dw_low_date_time as u64
}

#[cfg(windows)]
#[repr(C)]
struct MemoryStatusEx {
    length: u32,
    memory_load: u32,
    total_phys: u64,
    avail_phys: u64,
    total_page_file: u64,
    avail_page_file: u64,
    total_virtual: u64,
    avail_virtual: u64,
    avail_extended_virtual: u64,
}

#[cfg(windows)]
const PROCESS_MODE_BACKGROUND_BEGIN: u32 = 0x0010_0000;

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCurrentProcess() -> *mut core::ffi::c_void;
    fn SetPriorityClass(process: *mut core::ffi::c_void, flags: u32) -> i32;
    fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    fn GetLastError() -> u32;
}

#[cfg(test)]
mod tests {
    use super::BackgroundLimits;

    // ============================================================================
    // しきい値の範囲外を拒否することを確認する。
    // ============================================================================
    #[test]
    fn rejects_invalid_limits() {
        let mut limits = BackgroundLimits::default();
        limits.cpu_busy_percent = 0;
        assert!(limits.validate().is_err());
        limits.cpu_busy_percent = 70;
        limits.memory_load_percent = 101;
        assert!(limits.validate().is_err());
    }
}
