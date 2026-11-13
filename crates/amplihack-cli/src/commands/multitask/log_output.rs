//! Bounded child output capture driven by pipe closure.
//! Both pipes are drained even after the capture limit, so verbose children
//! cannot deadlock or grow logs without bound.
use anyhow::{Context, Result, ensure};
use std::{
    fs::File,
    io::{Read, Write},
    sync::mpsc,
    thread,
};

#[derive(Debug)]
pub(super) struct Capture {
    drains: Vec<mpsc::Receiver<std::io::Result<()>>>,
}

impl Capture {
    pub(super) fn start(
        stdout: impl Read + Send + 'static,
        stderr: impl Read + Send + 'static,
        output: File,
        log: File,
    ) -> Self {
        let limit = std::env::var("AMPLIHACK_MAX_LOG_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(100 * 1024 * 1024);
        let drains = vec![
            drain(stdout, output, limit, true),
            drain(stderr, log, limit, false),
        ];
        Self { drains }
    }

    pub(super) fn finish(self) -> Result<()> {
        for drain in self.drains {
            drain
                .recv()
                .context("child output observer disconnected")??;
        }
        Ok(())
    }
}

fn drain(
    mut input: impl Read + Send + 'static,
    mut output: File,
    limit: u64,
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
        let done = drain(
            std::io::Cursor::new(vec![b'x'; 20000]),
            File::create(&path).unwrap(),
            4,
            false,
        );
        done.recv().unwrap().unwrap();
        let log = std::fs::read_to_string(path).unwrap();
        assert!(log.starts_with("xxxx\n[truncated: discarded 19996 bytes"));
        assert!(log.len() < 100);
    }
}
