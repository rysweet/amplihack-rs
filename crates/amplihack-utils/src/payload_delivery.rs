//! Helpers for handing large text payloads to child processes (issue #1570).
//!
//! Linux caps a single argv or environment string at `MAX_ARG_STRLEN`
//! (131,072 bytes), so unbounded text must reach a child on stdin or in a
//! file. This module provides the stdin writers (blocking and tokio) used by
//! every such site and UTF-8-safe byte truncation for inputs that must stay
//! bounded anyway.
//!
//! See `docs/reference/large-payload-delivery.md`.

use std::borrow::Cow;
use std::io::{self, Write};
use std::process::ChildStdin;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Marker placed before a tail kept by [`truncate_tail_with_marker`].
pub const TRUNCATION_MARKER: &str = "[... earlier output truncated ...]\n";

/// A background thread writing a payload to a child's stdin.
///
/// The write runs on its own thread so that a child which produces output
/// before reading its input cannot deadlock against the parent.
#[derive(Debug)]
pub struct StdinWriter {
    handle: JoinHandle<io::Result<()>>,
}

/// Starts a thread that writes `payload` to `stdin` and then closes it.
pub fn spawn_stdin_writer(stdin: ChildStdin, payload: Vec<u8>) -> StdinWriter {
    spawn_writer(stdin, payload, None, Vec::new())
}

/// Starts a thread that writes `payload` to `stdin`, then copies `follow`
/// to it until `follow` reaches end of file, then closes it.
///
/// Used when a prompt takes the child's stdin but the caller also piped
/// input to the parent: the child gets the prompt first and the piped input
/// after it, as one stream. The thread blocks on `follow` for as long as its
/// writer keeps it open, exactly as the child would have done reading it
/// directly; see [`StdinWriter::finish_if_done`].
pub fn spawn_stdin_forwarder(
    stdin: ChildStdin,
    payload: Vec<u8>,
    follow: Box<dyn io::Read + Send>,
) -> StdinWriter {
    spawn_writer(stdin, payload, Some(follow), Vec::new())
}

/// Starts a thread that writes `head`, copies `follow` until it reaches end
/// of file, writes `tail`, and then closes `stdin`.
///
/// Used when the caller's own stdin is the prompt and text must follow it
/// (issue #2535: operator guidance after a Codex `exec … -` prompt). Like
/// [`spawn_stdin_forwarder`], nothing of `follow` is buffered beyond the
/// copy and nothing is timed; a `BrokenPipe` is not an error and errors
/// never include payload bytes.
pub fn spawn_stdin_forwarder_with_trailer(
    stdin: ChildStdin,
    head: Vec<u8>,
    follow: Box<dyn io::Read + Send>,
    tail: Vec<u8>,
) -> StdinWriter {
    spawn_writer(stdin, head, Some(follow), tail)
}

fn spawn_writer(
    stdin: ChildStdin,
    payload: Vec<u8>,
    follow: Option<Box<dyn io::Read + Send>>,
    tail: Vec<u8>,
) -> StdinWriter {
    let handle = std::thread::spawn(move || {
        let mut stdin = stdin;
        let result = stdin
            .write_all(&payload)
            .and_then(|()| match follow {
                Some(mut source) => io::copy(&mut source, &mut stdin).map(drop),
                None => Ok(()),
            })
            .and_then(|()| stdin.write_all(&tail))
            .and_then(|()| stdin.flush());
        // Dropping `stdin` closes the pipe, which gives the child EOF.
        drop(stdin);
        match result {
            // The child exited or closed stdin without reading everything.
            // Its exit status, not the write, decides the outcome.
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
                ) =>
            {
                Ok(())
            }
            // Never include payload bytes in the error.
            Err(e) => Err(io::Error::new(
                e.kind(),
                format!("writing payload to child stdin failed: {e}"),
            )),
            Ok(()) => Ok(()),
        }
    });
    StdinWriter { handle }
}

impl StdinWriter {
    /// Joins the writer thread.
    ///
    /// Returns `Ok` when every byte was written or the child closed stdin
    /// first (`BrokenPipe` or `ConnectionReset`), and an error of kind `Other` if the thread
    /// panicked.
    pub fn finish(self) -> io::Result<()> {
        match self.handle.join() {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("stdin writer thread panicked")),
        }
    }

    /// Joins the writer thread if it has already ended, and otherwise
    /// detaches it and returns `None`.
    ///
    /// Call this after the child has exited. A forwarder from
    /// [`spawn_stdin_forwarder`] can still be blocked reading a source whose
    /// writer never closes it; the child is gone, so nothing it could still
    /// copy has a reader, and the thread ends with the process.
    pub fn finish_if_done(self) -> Option<io::Result<()>> {
        self.handle.is_finished().then(|| self.finish())
    }

    /// Joins the writer thread if it finishes within `grace`.
    ///
    /// Returns `None`, detaching the thread, when it is still running at the
    /// end of `grace`. That happens when the child exited but a descendant
    /// still holds stdin open without reading it; the thread then ends once
    /// that descendant closes the pipe.
    pub fn finish_within(self, grace: Duration) -> Option<io::Result<()>> {
        let deadline = Instant::now() + grace;
        while !self.handle.is_finished() {
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Some(self.finish())
    }
}

/// How a [`write_stdin_async`] write ended without an I/O error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinDelivery {
    /// Every byte was written and stdin was closed.
    Complete,
    /// The child closed its end before reading everything.
    ReaderClosed,
}

/// Writes `chunks` in order to a tokio child's stdin, then flushes, shuts
/// down and drops it, giving the child EOF.
///
/// The chunks are written back to back with no separator. A `BrokenPipe` or
/// `ConnectionReset` yields [`StdinDelivery::ReaderClosed`]; the caller
/// decides what that means. Other errors keep their kind and never include
/// payload bytes.
pub async fn write_stdin_async(
    mut stdin: tokio::process::ChildStdin,
    chunks: &[&[u8]],
) -> io::Result<StdinDelivery> {
    use tokio::io::AsyncWriteExt;
    let result = async {
        for chunk in chunks {
            stdin.write_all(chunk).await?;
        }
        stdin.flush().await?;
        stdin.shutdown().await
    }
    .await;
    drop(stdin);
    match result {
        Ok(()) => Ok(StdinDelivery::Complete),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::BrokenPipe | io::ErrorKind::ConnectionReset
            ) =>
        {
            Ok(StdinDelivery::ReaderClosed)
        }
        Err(e) => Err(io::Error::new(
            e.kind(),
            format!("writing payload to child stdin failed: {e}"),
        )),
    }
}

/// Keeps the first `max` bytes of `s`, cut back to a UTF-8 boundary.
pub fn truncate_head_bytes(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    Cow::Owned(s[..end].to_owned())
}

/// Keeps the last `max` bytes of `s`, cut forward to a UTF-8 boundary.
pub fn truncate_tail_bytes(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    Cow::Owned(s[start..].to_owned())
}

/// Keeps the tail of `s` behind [`TRUNCATION_MARKER`]; the result, marker
/// included, is at most `max` bytes. When `max` cannot hold the marker plus
/// any text, the bare tail is returned instead.
pub fn truncate_tail_with_marker(s: &str, max: usize) -> Cow<'_, str> {
    if s.len() <= max {
        return Cow::Borrowed(s);
    }
    if max <= TRUNCATION_MARKER.len() {
        return truncate_tail_bytes(s, max);
    }
    let tail = truncate_tail_bytes(s, max - TRUNCATION_MARKER.len());
    let mut out = String::with_capacity(TRUNCATION_MARKER.len() + tail.len());
    out.push_str(TRUNCATION_MARKER);
    out.push_str(&tail);
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn head_and_tail_respect_char_boundaries() {
        assert_eq!(truncate_head_bytes("aé", 2), "a");
        assert_eq!(truncate_tail_bytes("éa", 2), "a");
    }

    #[test]
    fn marker_is_used_only_when_text_is_dropped() {
        assert_eq!(truncate_tail_with_marker("short", 100), "short");
        let long = "x".repeat(200);
        let kept = truncate_tail_with_marker(&long, 100);
        assert!(kept.starts_with(TRUNCATION_MARKER));
        assert_eq!(kept.len(), 100);
    }

    // Issue #2535: the Codex `exec … -` form gets operator guidance after the
    // caller's stdin closes. `spawn_stdin_forwarder_with_trailer` writes the
    // head, copies `follow` to end of file, then writes the trailer.

    fn cat_child() -> std::process::Child {
        std::process::Command::new("cat")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("start cat")
    }

    #[test]
    fn issue_2535_trailer_follows_head_and_caller_stdin() {
        let mut child = cat_child();
        let stdin = child.stdin.take().unwrap();
        let writer = spawn_stdin_forwarder_with_trailer(
            stdin,
            b"HEAD|".to_vec(),
            Box::new(io::Cursor::new(b"FOLLOW|".to_vec())),
            b"TAIL".to_vec(),
        );
        writer.finish().expect("write succeeds");
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "HEAD|FOLLOW|TAIL");
    }

    #[test]
    fn issue_2535_empty_head_and_follow_give_the_trailer_only() {
        let mut child = cat_child();
        let stdin = child.stdin.take().unwrap();
        let writer = spawn_stdin_forwarder_with_trailer(
            stdin,
            Vec::new(),
            Box::new(io::empty()),
            b"TRAILER ONLY".to_vec(),
        );
        writer.finish().expect("write succeeds");
        let out = child.wait_with_output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), "TRAILER ONLY");
    }

    #[test]
    fn issue_2535_empty_trailer_matches_the_plain_forwarder() {
        let mut child = cat_child();
        let stdin = child.stdin.take().unwrap();
        let writer = spawn_stdin_forwarder_with_trailer(
            stdin,
            b"prompt\n".to_vec(),
            Box::new(io::Cursor::new(b"piped\n".to_vec())),
            Vec::new(),
        );
        writer.finish().unwrap();
        let out = child.wait_with_output().unwrap();
        assert_eq!(out.stdout, b"prompt\npiped\n");
    }

    #[test]
    fn issue_2535_trailer_broken_pipe_is_not_an_error_and_names_no_payload() {
        // The child exits without reading: the trailer write hits EPIPE.
        let mut child = std::process::Command::new("true")
            .stdin(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        child.wait().unwrap();
        let big = vec![b'x'; 1 << 20];
        let writer =
            spawn_stdin_forwarder_with_trailer(stdin, Vec::new(), Box::new(io::empty()), big);
        // BrokenPipe means the child stopped reading; its exit status decides.
        if let Err(e) = writer.finish() {
            assert!(!e.to_string().contains("xxxx"), "payload bytes in error");
            panic!("a broken pipe must not be an error: {e}");
        }
    }
}
