//! ============================================================================
//! Git 風の進捗表示（stderr、TTY のときだけ更新する）。
//!
//! フェーズ開始 → 同一行の `\r` 更新 → 完了行、という流れに揃える。
//! パイプやリダイレクト時は黙る（verify の従来挙動と同じ）。
//! ============================================================================

use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);

// ============================================================================
// ターミナル向けの進捗レポーター。
// ============================================================================
pub struct Progress {
    writer: Box<dyn Write>,
    enabled: bool,
    phase: String,
    last_update: Instant,
}

impl Progress {
    // ============================================================================
    // stderr を使い、TTY のときだけ表示する。
    // ============================================================================
    pub fn stderr_if_tty() -> Self {
        Self {
            writer: Box::new(io::stderr()),
            enabled: io::stderr().is_terminal(),
            phase: String::new(),
            last_update: Instant::now() - UPDATE_INTERVAL,
        }
    }

    // ============================================================================
    // 表示しない（テストや非対話実行向け）。
    // ============================================================================
    pub fn quiet() -> Self {
        Self {
            writer: Box::new(io::sink()),
            enabled: false,
            phase: String::new(),
            last_update: Instant::now(),
        }
    }

    // ============================================================================
    // フェーズを開始する。Git と同様に末尾に `...` を付ける。
    // ============================================================================
    pub fn begin(&mut self, phase: &str) {
        self.phase = phase.to_string();
        if !self.enabled {
            return;
        }
        let _ = writeln!(self.writer, "{phase}...");
        let _ = self.writer.flush();
        self.last_update = Instant::now();
    }

    // ============================================================================
    // 総数が分かるときの更新（例: Restoring files:  45% (123/456)）。
    // ============================================================================
    pub fn ratio(&mut self, current: usize, total: usize) {
        if !self.enabled || total == 0 {
            return;
        }
        let now = Instant::now();
        let at_end = current >= total;
        if !at_end && now.duration_since(self.last_update) < UPDATE_INTERVAL {
            return;
        }
        self.last_update = now;

        let pct = current.saturating_mul(100) / total;
        let _ = write!(
            self.writer,
            "\r{}: {pct:3}% ({current}/{total})",
            self.phase
        );
        let _ = self.writer.flush();
        if at_end {
            let _ = writeln!(self.writer, ", done.");
        }
    }

    // ============================================================================
    // 総数が不明なときの更新（走査中のエントリ数など）。
    // ============================================================================
    pub fn count(&mut self, current: usize, detail: &str) {
        if !self.enabled {
            return;
        }
        let now = Instant::now();
        if now.duration_since(self.last_update) < UPDATE_INTERVAL {
            return;
        }
        self.last_update = now;
        let _ = write!(self.writer, "\r{}: {current} {detail}", self.phase);
        let _ = self.writer.flush();
    }

    // ============================================================================
    // フェーズを完了行で締める（`\r` 行のあとに改行付きサマリを出す）。
    // ============================================================================
    pub fn done(&mut self, summary: &str) {
        if self.enabled {
            let _ = writeln!(self.writer, "{}: {summary}", self.phase);
            let _ = self.writer.flush();
        }
        self.phase.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::Progress;

    // ============================================================================
    // quiet は進捗 API を呼んでも panic しないことを確認する。
    // ============================================================================
    #[test]
    fn quiet_progress_is_silent() {
        let mut progress = Progress::quiet();
        progress.begin("Scanning files");
        progress.count(10, "entries");
        progress.ratio(1, 5);
        progress.done("10 entries, done.");
    }
}
