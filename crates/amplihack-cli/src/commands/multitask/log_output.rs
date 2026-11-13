//! Bounded child output capture and activity tracking for runtime policies.
//! Both pipes are drained even after the capture limit, so verbose children
//! cannot deadlock or grow logs without bound. Activity is tracked independently
//! of capture size and includes output without newlines.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};

#[derive(Debug)]
pub(super) struct Capture {
    activity: Arc<Mutex<Instant>>,
    drains: Vec<mpsc::Receiver<std::io::Result<()>>>,
}

impl Capture {
    pub(super) fn start(
        stdout: impl Read + Send + 'static,
        stderr: impl Read + Send + 'static,
        output: File,
        log: File,
    ) -> Self {
        let activity = Arc::new(Mutex::new(Instant::now()));
        let limit = std::env::var("AMPLIHACK_MAX_LOG_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(100 * 1024 * 1024);
        let drains = vec![
            drain(stdout, output, limit, activity.clone(), true),
            drain(stderr, log, limit, activity.clone(), false),
        ];
        Self { activity, drains }
    }

    pub(super) fn idle_remaining(&self) -> Duration {
        amplihack_utils::idle_watchdog::IdleConfig::from_env()
            .idle_timeout
            .saturating_sub(
                self.activity
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed(),
            )
    }

    pub(super) fn finish(self) -> Result<()> {
        for drain in self.drains {
            drain
                .recv_timeout(Duration::from_secs(5))
                .context("child output did not close after exit; reconcile owned descendants")??;
        }
        Ok(())
    }
}

fn drain(
    mut input: impl Read + Send + 'static,
    mut output: File,
    limit: u64,
    activity: Arc<Mutex<Instant>>,
    result_output: bool,
) -> mpsc::Receiver<std::io::Result<()>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = (|| {
            let mut buffer = [0; 8192];
            let mut retained = 0;
            let mut discarded = 0u64;
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                *activity.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
                let keep = (limit.saturating_sub(retained)).min(count as u64) as usize;
                output.write_all(&buffer[..keep])?;
                retained += keep as u64;
                discarded = discarded.saturating_add((count - keep) as u64);
            }
            if discarded > 0 {
                writeln!(
                    output,
                    "\n[truncated: discarded {discarded} bytes after log capture limit]"
                )?;
            }
            output.flush()?;
            // Truncated result JSON is never sufficient evidence of success.
            ensure!(
                !result_output || discarded == 0,
                "runner result exceeded output capture limit"
            );
            Ok::<_, anyhow::Error>(())
        })();
        let _ = tx.send(result.map_err(std::io::Error::other));
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_capture_drains_input_and_reports_discarded_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("log");
        let activity = Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1)));
        let done = drain(
            std::io::Cursor::new(vec![b'x'; 20000]),
            File::create(&path).unwrap(),
            4,
            activity.clone(),
            false,
        );
        done.recv().unwrap().unwrap();
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.starts_with("xxxx\n[truncated: discarded 19996 bytes"));
        assert!(log.len() < 100);
        assert!(activity.lock().unwrap().elapsed() < Duration::from_secs(1));
    }
}
