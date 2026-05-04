//! Pre-commit-prefs hook: drains stdin and exits successfully (no-op).
//!
//! Some `settings.json` templates and external git pre-commit wrappers wire
//! a "snapshot user preferences before commit" hook at this lifecycle
//! moment. The amplihack-hooks native binary does not currently expose
//! a dedicated subcommand for that work, so this handler drains stdin
//! (so the parent process does not block on a full pipe) and exits 0
//! with no stdout — the canonical no-op response.
//!
//! Security: this handler must NEVER log, echo, parse, or persist the
//! stdin payload. Pre-commit input may carry user prompts or secrets.
//! If a future native subcommand is added, switching this no-op to
//! forward to it is a one-line change.

use std::io::{self, Read};

/// Drain `input` until EOF and return success.
///
/// The handler intentionally discards every byte read. It does not
/// log, parse, or echo the payload (security: stdin may contain
/// secrets — see crate-level docs).
pub fn run<R: Read>(input: &mut R) -> io::Result<()> {
    // 64 KiB scratch buffer — sized to amortise read syscalls without
    // holding the entire payload in memory at once. We discard each
    // chunk immediately, so peak memory usage stays bounded regardless
    // of stdin size.
    let mut scratch = [0u8; 64 * 1024];
    loop {
        match input.read(&mut scratch) {
            Ok(0) => return Ok(()),
            Ok(_) => continue,
            Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
            // Broken pipe is not an error for a no-op drain — the parent
            // closed the pipe before we finished consuming. Treat as EOF.
            Err(ref e) if e.kind() == io::ErrorKind::BrokenPipe => return Ok(()),
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::run;
    use std::io::Cursor;

    #[test]
    fn run_drains_stdin_and_returns_ok_with_empty_input() {
        let mut input = Cursor::new(Vec::<u8>::new());
        assert!(run(&mut input).is_ok());
    }

    #[test]
    fn run_drains_stdin_and_returns_ok_with_payload() {
        let mut input = Cursor::new(b"{\"any\":\"payload\"}".to_vec());
        assert!(run(&mut input).is_ok());
    }

    #[test]
    fn run_consumes_entire_stdin() {
        let payload = b"hello-world".to_vec();
        let len = payload.len() as u64;
        let mut input = Cursor::new(payload);
        run(&mut input).unwrap();
        assert_eq!(input.position(), len, "must consume entire payload");
    }

    #[test]
    fn run_tolerates_large_input() {
        let payload = vec![b'x'; 1024 * 1024];
        let mut input = Cursor::new(payload);
        assert!(run(&mut input).is_ok(), "1 MiB stdin must succeed");
    }

    #[test]
    fn run_propagates_non_interrupted_errors() {
        // A reader that returns a hard IO error (not Interrupted/BrokenPipe)
        // must surface as Err — we don't silently swallow real failures.
        struct FailingReader;
        impl std::io::Read for FailingReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "denied",
                ))
            }
        }
        let mut r = FailingReader;
        assert!(run(&mut r).is_err());
    }

    #[test]
    fn run_treats_broken_pipe_as_eof() {
        struct BrokenPipeReader(bool);
        impl std::io::Read for BrokenPipeReader {
            fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
                if self.0 {
                    Ok(0)
                } else {
                    self.0 = true;
                    Err(std::io::Error::new(std::io::ErrorKind::BrokenPipe, "pipe"))
                }
            }
        }
        let mut r = BrokenPipeReader(false);
        assert!(run(&mut r).is_ok());
    }
}
