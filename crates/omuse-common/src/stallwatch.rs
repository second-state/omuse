//! Event-loop stall watchdog: when the async runtime does not tick for `stall_secs` seconds (a blocking call in
//! async code), write a report to `<data>/loop_stalls.log` so the culprit can be found. While the loop is blocked the
//! HTTP server cannot accept connections, and Sentinel answers the UI with 503 "runtime unavailable".
//!
//! Port of `app/common/stallwatch.py`. The Python version dumped the loop thread's Python stack; a Rust stack trace
//! of another thread is not available, so the report carries the lag line and `(no frame)` instead. The threshold
//! comes from the caller (config `server.loop_stall_seconds`), not from an environment variable.

use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::util::{monotonic, take_chars};

/// Rotate the log once it grows past this many bytes (keeps it small).
const MAX_LOG_BYTES: u64 = 2_000_000;

/// Start the watchdog for the current Tokio runtime: a heartbeat task records the monotonic time every 0.5 s and a
/// `stallwatch` OS thread checks once a second whether the heartbeat is older than `stall_secs`. One report is written
/// per stall. Must be called from within a Tokio runtime (it spawns the heartbeat task).
pub fn start(data_dir: &Path, name: &str, stall_secs: f64) {
    let beat = Arc::new(AtomicU64::new(monotonic().to_bits()));
    let path = data_dir.join("loop_stalls.log");

    let hb = Arc::clone(&beat);
    tokio::spawn(async move {
        loop {
            hb.store(monotonic().to_bits(), Ordering::Relaxed);
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });

    let name = name.to_string();
    let spawned = std::thread::Builder::new()
        .name("stallwatch".into())
        .spawn(move || {
            let mut reported = f64::NAN; // never equal to a real heartbeat → first stall is reported
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let last = f64::from_bits(beat.load(Ordering::Relaxed));
                let lag = monotonic() - last;
                if lag < stall_secs || last == reported {
                    continue;
                }
                reported = last; // one report per stall
                let stack = "(no frame)";
                let line = format!(
                    "{} {} event loop blocked {:.1}s+\n{}\n",
                    chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
                    name,
                    lag,
                    stack
                );
                if let Err(e) = append_report(&path, &line) {
                    let _ = e; // like the Python `except OSError: pass`
                }
                eprintln!("{}", take_chars(&line, 1500));
            }
        });
    if let Err(e) = spawned {
        eprintln!("[stallwatch] could not start watchdog thread: {e}");
    }
}

fn append_report(path: &Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)?;
    f.write_all(line.as_bytes())?;
    drop(f);
    if std::fs::metadata(path)?.len() > MAX_LOG_BYTES {
        let mut rotated = path.as_os_str().to_owned();
        rotated.push(".1");
        std::fs::rename(path, rotated)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `test_stallwatch_reports_a_blocked_loop` from tests/test_units.py: a blocking sleep inside async code is
    /// reported in `loop_stalls.log`. Uses a single-threaded runtime, the equivalent of the Python event loop.
    #[test]
    fn stallwatch_reports_a_blocked_loop() {
        let dir =
            std::env::temp_dir().join(format!("omuse-stallwatch-{}", crate::util::uuid_hex()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            start(&dir, "test", 0.8);
            tokio::time::sleep(Duration::from_millis(600)).await;
            std::thread::sleep(Duration::from_millis(2500)); // a blocking call inside async code
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        // the watchdog thread polls once a second; give it a moment to write the report
        let path = dir.join("loop_stalls.log");
        let mut log = String::new();
        for _ in 0..30 {
            log = std::fs::read_to_string(&path).unwrap_or_default();
            if log.contains("test event loop blocked") {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(log.contains("test event loop blocked"), "log was: {log:?}");
        assert!(log.contains("(no frame)"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
