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

/// システム全体のネットワーク使用量がこれを超えたら待機する既定値（KiB/s）。
pub const DEFAULT_NETWORK_BUSY_KBPS: u32 = 8_192;

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
    pub network_busy_kbps: u32,
    pub max_transfer_kbps: u32,
}

impl Default for BackgroundLimits {
    fn default() -> Self {
        Self {
            cpu_busy_percent: DEFAULT_CPU_BUSY_PERCENT,
            memory_load_percent: DEFAULT_MEMORY_LOAD_PERCENT,
            poll_interval: DEFAULT_POLL_INTERVAL,
            network_busy_kbps: DEFAULT_NETWORK_BUSY_KBPS,
            max_transfer_kbps: 0,
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
            let network_busy =
                sample.network_kbps > self.limits.network_busy_kbps;
            if !cpu_busy && !memory_busy && !network_busy {
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
        if self.limits.max_transfer_kbps > 0 {
            let bytes_per_second = self.limits.max_transfer_kbps as u64 * 1024;
            let delay_ms = (bytes as u64).saturating_mul(1000) / bytes_per_second.max(1);
            if delay_ms > 0 {
                thread::sleep(Duration::from_millis(delay_ms));
            }
        }

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
    network_kbps: u32,
}

// ============================================================================
// OS から CPU / メモリ / ネットワークを読む。プラットフォームごとに実装を分ける。
// ============================================================================
struct ResourceSampler {
    #[cfg(windows)]
    previous: Option<WindowsCpuTimes>,
    #[cfg(not(windows))]
    previous_cpu: Option<(u64, u64)>,
    previous_network_bytes: Option<u64>,
    previous_sample_at: Option<std::time::Instant>,
}

impl ResourceSampler {
    fn new() -> Result<Self> {
        let mut sampler = Self {
            #[cfg(windows)]
            previous: None,
            #[cfg(not(windows))]
            previous_cpu: None,
            previous_network_bytes: None,
            previous_sample_at: None,
        };
        // 初回は差分が取れないので、短い間隔で 2 度読んで基準を作る。
        let _ = sampler.sample()?;
        thread::sleep(Duration::from_millis(50));
        let _ = sampler.sample()?;
        Ok(sampler)
    }

    fn sample(&mut self) -> Result<ResourceSample> {
        let now = std::time::Instant::now();
        let memory_load_percent = memory_load_percent()?;
        let cpu_busy_percent = self.cpu_busy_percent()?;
        let network_bytes = network_bytes_total()?;
        let network_kbps = match (self.previous_network_bytes, self.previous_sample_at) {
            (Some(previous_bytes), Some(previous_at)) => {
                let elapsed = now.duration_since(previous_at).as_secs_f64();
                if elapsed <= 0.0 {
                    0
                } else {
                    let delta = network_bytes.saturating_sub(previous_bytes);
                    ((delta as f64) / elapsed / 1024.0) as u32
                }
            }
            _ => 0,
        };
        self.previous_network_bytes = Some(network_bytes);
        self.previous_sample_at = Some(now);
        Ok(ResourceSample {
            cpu_busy_percent,
            memory_load_percent,
            network_kbps,
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

// ============================================================================
// 全インターフェースの送受信バイト合計を返す。
// ============================================================================
fn network_bytes_total() -> Result<u64> {
    #[cfg(windows)]
    {
        read_windows_network_bytes()
    }
    #[cfg(not(windows))]
    {
        let text =
            std::fs::read_to_string("/proc/net/dev").context("failed to read /proc/net/dev")?;
        let mut total = 0_u64;
        for line in text.lines().skip(2) {
            let mut parts = line.split_whitespace();
            let Some(_iface) = parts.next() else {
                continue;
            };
            let receive = parts
                .next()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let transmit = parts
                .nth(7)
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            total = total.saturating_add(receive).saturating_add(transmit);
        }
        Ok(total)
    }
}

#[cfg(windows)]
fn read_windows_network_bytes() -> Result<u64> {
    use std::mem::size_of;

    #[repr(C)]
    struct MibIfRow {
        name: [u16; 256],
        index: u32,
        type_: u32,
        mtu: u32,
        speed: u32,
        phys_addr_len: u32,
        phys_addr: [u8; 8],
        admin_status: u32,
        oper_status: u32,
        last_change: u32,
        in_octets: u32,
        in_ucast_pkts: u32,
        in_nucast_pkts: u32,
        in_discards: u32,
        in_errors: u32,
        in_unknown_protos: u32,
        out_octets: u32,
        out_ucast_pkts: u32,
        out_nucast_pkts: u32,
        out_discards: u32,
        out_errors: u32,
        out_qlen: u32,
        descr: u32,
    }

    #[repr(C)]
    struct MibIfTable {
        count: u32,
        table: [MibIfRow; 1],
    }

    let mut size = 0_u32;
    unsafe {
        let status = GetIfTable(std::ptr::null_mut(), &mut size, 0);
        if status != ERROR_INSUFFICIENT_BUFFER {
            bail!("GetIfTable size query failed with status {status}");
        }
        let mut buffer = vec![0_u8; size as usize];
        let status = GetIfTable(buffer.as_mut_ptr().cast(), &mut size, 0);
        if status != NO_ERROR {
            bail!("GetIfTable failed with status {status}");
        }
        let table = &*(buffer.as_ptr().cast::<MibIfTable>());
        let count = table.count as usize;
        let base = buffer.as_ptr().cast::<u8>();
        let row_size = size_of::<MibIfRow>();
        let mut total = 0_u64;
        for index in 0..count {
            let row = &*(base.add(size_of::<u32>() + index * row_size).cast::<MibIfRow>());
            total = total
                .saturating_add(row.in_octets as u64)
                .saturating_add(row.out_octets as u64);
        }
        Ok(total)
    }
}

#[cfg(windows)]
const NO_ERROR: u32 = 0;
#[cfg(windows)]
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

#[cfg(windows)]
#[link(name = "iphlpapi")]
unsafe extern "system" {
    fn GetIfTable(table: *mut core::ffi::c_void, size: *mut u32, order: i32) -> u32;
}

#[cfg(test)]
mod tests {
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{BackgroundLimits, activate};
    use crate::pace::IoPace;

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

    fn try_activate(limits: BackgroundLimits) -> Option<super::BackgroundPace> {
        activate(limits).ok()
    }

    // ============================================================================
    // 低優先度モードへ入り、資源サンプリングが動くことを確認する。
    // ============================================================================
    #[test]
    fn activate_and_pace_smoke() {
        let mut limits = BackgroundLimits::default();
        limits.cpu_busy_percent = 100;
        limits.memory_load_percent = 100;
        limits.network_busy_kbps = u32::MAX;
        limits.max_transfer_kbps = 512;
        let Some(mut pace) = try_activate(limits) else {
            return;
        };
        pace.before_entry()
            .expect("resource sampling should succeed");
        pace.after_chunk(1024)
            .expect("paced chunk should succeed");
    }

    // ============================================================================
    // 転送速度上限が処理を遅延させることを確認する。
    // ============================================================================
    #[test]
    fn max_transfer_kbps_throttles_chunks() {
        let mut limits = BackgroundLimits::default();
        limits.cpu_busy_percent = 100;
        limits.memory_load_percent = 100;
        limits.network_busy_kbps = u32::MAX;
        limits.max_transfer_kbps = 64;
        let Some(mut pace) = try_activate(limits) else {
            return;
        };
        let started = Instant::now();
        pace.after_chunk(64 * 1024)
            .expect("paced chunk should succeed");
        assert!(
            started.elapsed() >= Duration::from_millis(900),
            "expected transfer throttle delay, got {:?}",
            started.elapsed()
        );
    }

    // ============================================================================
    // 高 CPU しきい値なら短時間の負荷下でも before_entry が返ることを確認する。
    // ============================================================================
    #[test]
    fn high_cpu_threshold_does_not_block_on_brief_load() {
        let mut limits = BackgroundLimits::default();
        limits.cpu_busy_percent = 100;
        limits.memory_load_percent = 100;
        limits.network_busy_kbps = u32::MAX;
        let Some(mut pace) = try_activate(limits) else {
            return;
        };
        let load = thread::spawn(|| {
            let start = Instant::now();
            let mut acc = 0_u32;
            while start.elapsed() < Duration::from_millis(200) {
                for value in 0..10_000u32 {
                    acc = acc.wrapping_add(value.wrapping_mul(value));
                }
            }
            std::hint::black_box(acc);
        });
        let started = Instant::now();
        pace.before_entry()
            .expect("before_entry should succeed under brief load with 100% threshold");
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "before_entry blocked too long: {:?}",
            started.elapsed()
        );
        load.join().expect("load thread should finish");
    }
}
