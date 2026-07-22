//! ============================================================================
//! 進捗表示（stderr）と結果表示（stdout）を分ける。
//!
//! - 進捗: このモジュール経由で stderr へ出す（フェーズ・比率・注記）
//! - 結果: 呼び出し側が stdout（println!）へ出す
//! - 区切り: 進捗の最後に `end()` で空行を stderr に出し、結果と視覚的に分ける
//!
//! フェーズ行は TTY でなくても出す。同一行 `\r` 更新は TTY のときだけ。
//! ============================================================================

use std::{
    io::{self, IsTerminal, Write},
    time::{Duration, Instant},
};

const UPDATE_INTERVAL: Duration = Duration::from_millis(100);
const NON_TTY_STEP_INTERVAL: Duration = Duration::from_secs(1);

// ============================================================================
// ターミナル向けの進捗レポーター（出力先は stderr）。
// ============================================================================
pub struct Progress {
    writer: Box<dyn Write>,
    /// `\r` による同一行更新を行う。
    interactive: bool,
    phase: String,
    last_update: Instant,
    /// begin 以降に何か書いたら end() で空行を足す。
    wrote_anything: bool,
}

impl Progress {
    // ============================================================================
    // CLI 向け。フェーズ行は常に出し、詳細更新は TTY のときだけ同一行更新。
    // ============================================================================
    pub fn stderr_for_cli() -> Self {
        Self {
            writer: Box::new(io::stderr()),
            interactive: io::stderr().is_terminal(),
            phase: String::new(),
            last_update: Instant::now() - UPDATE_INTERVAL,
            wrote_anything: false,
        }
    }

    // ============================================================================
    // 表示しない（テストや非対話実行向け）。
    // ============================================================================
    pub fn quiet() -> Self {
        Self {
            writer: Box::new(io::sink()),
            interactive: false,
            phase: String::new(),
            last_update: Instant::now(),
            wrote_anything: false,
        }
    }

    // ============================================================================
    // フェーズを開始する。Git と同様に末尾に `...` を付ける。
    // ============================================================================
    pub fn begin(&mut self, phase: &str) {
        self.phase = phase.to_string();
        let _ = writeln!(self.writer, "{phase}...");
        let _ = self.writer.flush();
        self.last_update = Instant::now();
        self.wrote_anything = true;
    }

    // ============================================================================
    // 1 件の処理を始めるときに呼ぶ。TTY 以外でも行を出す。
    // ============================================================================
    pub fn step(&mut self, current: usize, total: usize, detail: &str) {
        if total == 0 {
            return;
        }
        let _ = writeln!(
            self.writer,
            "{}: [{current}/{total}] {detail}",
            self.phase
        );
        let _ = self.writer.flush();
        self.last_update = Instant::now();
        self.wrote_anything = true;
    }

    // ============================================================================
    // 総数が分かるときの更新（例: Restoring files:  45% (123/456)）。
    // ============================================================================
    pub fn ratio(&mut self, current: usize, total: usize) {
        if total == 0 {
            return;
        }
        let now = Instant::now();
        let at_end = current >= total;
        if self.interactive {
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
            self.wrote_anything = true;
            if at_end {
                let _ = writeln!(self.writer, ", done.");
            }
            return;
        }

        if at_end || current == 1 || now.duration_since(self.last_update) >= NON_TTY_STEP_INTERVAL
        {
            self.last_update = now;
            let pct = current.saturating_mul(100) / total;
            let _ = writeln!(
                self.writer,
                "{}: {pct:3}% ({current}/{total})",
                self.phase
            );
            let _ = self.writer.flush();
            self.wrote_anything = true;
        }
    }

    // ============================================================================
    // 総数が不明なときの更新（走査中のエントリ数など）。
    // ============================================================================
    pub fn count(&mut self, current: usize, detail: &str) {
        let now = Instant::now();
        if self.interactive {
            if now.duration_since(self.last_update) < UPDATE_INTERVAL {
                return;
            }
            self.last_update = now;
            let _ = write!(self.writer, "\r{}: {current} {detail}", self.phase);
            let _ = self.writer.flush();
            self.wrote_anything = true;
            return;
        }

        if current == 1 || now.duration_since(self.last_update) >= NON_TTY_STEP_INTERVAL {
            self.last_update = now;
            let _ = writeln!(self.writer, "{}: {current} {detail}", self.phase);
            let _ = self.writer.flush();
            self.wrote_anything = true;
        }
    }

    // ============================================================================
    // フェーズを完了行で締める。
    // ============================================================================
    pub fn done(&mut self, summary: &str) {
        let _ = writeln!(self.writer, "{}: {summary}", self.phase);
        let _ = self.writer.flush();
        self.phase.clear();
        self.wrote_anything = true;
    }

    // ============================================================================
    // 進捗区間を終え、結果（stdout）とのあいだに空行を入れる。
    // ============================================================================
    pub fn end(&mut self) {
        if !self.wrote_anything {
            return;
        }
        // 同一行更新の途中なら改行してから空行へ。
        if self.interactive && !self.phase.is_empty() {
            let _ = writeln!(self.writer);
        }
        let _ = writeln!(self.writer);
        let _ = self.writer.flush();
        self.phase.clear();
        self.wrote_anything = false;
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
        time::Instant,
    };

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
        progress.step(1, 3, "item");
        progress.done("10 entries, done.");
        progress.end();
    }

    // ============================================================================
    // begin / step は writer へ必ず書くことを確認する。
    // ============================================================================
    #[test]
    fn begin_and_step_always_write_lines() {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        let capture = buffer.clone();
        let mut progress = Progress {
            writer: Box::new(WriterCapture { inner: capture }),
            interactive: false,
            phase: String::new(),
            last_update: Instant::now(),
            wrote_anything: false,
        };
        progress.begin("Restoring files");
        progress.step(1, 2, "abc12345");
        progress.done("2 objects, done.");
        progress.end();
        let output = String::from_utf8(buffer.lock().unwrap().clone()).unwrap();
        assert!(output.contains("Restoring files..."));
        assert!(output.contains("[1/2] abc12345"));
        assert!(output.contains("2 objects, done."));
        assert!(output.ends_with("\n\n") || output.contains("done.\n\n"));
    }

    struct WriterCapture {
        inner: Arc<Mutex<Vec<u8>>>,
    }

    impl Write for WriterCapture {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
