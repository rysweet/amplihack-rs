//! Tests for `amplihack_utils::payload_delivery` (issue #1570).
//!
//! Contract: docs/reference/large-payload-delivery.md, section
//! "Rust API: `amplihack_utils::payload_delivery`".
//!
//! Run with:
//!     cargo test -p amplihack-utils --test payload_delivery

#![cfg(unix)]

use std::borrow::Cow;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use amplihack_utils::payload_delivery::{
    StdinDelivery, TRUNCATION_MARKER, spawn_stdin_writer, truncate_head_bytes, truncate_tail_bytes,
    truncate_tail_with_marker, write_stdin_async,
};

const MIB: usize = 1024 * 1024;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Run `f` on a separate thread and fail the test if it does not finish
/// within `limit`. A deadlocked writer would otherwise hang the suite.
fn with_timeout<R: Send + 'static>(
    limit: Duration,
    what: &str,
    f: impl FnOnce() -> R + Send + 'static,
) -> R {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(limit) {
        Ok(r) => r,
        Err(_) => panic!("{what} did not complete within {limit:?} (deadlock?)"),
    }
}

/// A payload of exactly `size` bytes of valid UTF-8 mixing 1- to 4-byte
/// characters, so a byte-level corruption shows up as a mismatch.
fn utf8_payload(size: usize) -> Vec<u8> {
    let pattern = "payload 'q' \"d\" $X é € 中 𝄞\n";
    let mut out = String::with_capacity(size + pattern.len());
    while out.len() < size {
        out.push_str(pattern);
    }
    while out.len() > size {
        out.pop();
    }
    while out.len() < size {
        out.push('a');
    }
    out.into_bytes()
}

fn mixed_text() -> String {
    // 1-, 2-, 3- and 4-byte characters in every relative position.
    "a é € 𝄞 b 中é𝄞€ c ü🎉z".repeat(7)
}

// ---------------------------------------------------------------------------
// spawn_stdin_writer / StdinWriter::finish
// ---------------------------------------------------------------------------

#[test]
fn finish_is_ok_when_child_exits_without_reading_stdin() {
    with_timeout(Duration::from_secs(20), "non-reading child", || {
        let mut child = Command::new("true")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn true");
        let stdin = child.stdin.take().expect("piped stdin");
        let writer = spawn_stdin_writer(stdin, utf8_payload(MIB));
        let status = child.wait().expect("wait true");
        assert!(status.success());
        writer
            .finish()
            .expect("BrokenPipe from a child that never reads stdin must be non-fatal");
    });
}

#[test]
fn finish_is_ok_when_child_holds_stdin_open_without_reading_then_exits() {
    with_timeout(Duration::from_secs(20), "sleeping child", || {
        let mut child = Command::new("sh")
            .args(["-c", "sleep 0.3"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .expect("spawn sh");
        let stdin = child.stdin.take().expect("piped stdin");
        let writer = spawn_stdin_writer(stdin, utf8_payload(MIB));
        let status = child.wait().expect("wait sh");
        assert!(status.success());
        writer
            .finish()
            .expect("a pipe closed by an exiting child must map to Ok");
    });
}

#[test]
fn finish_is_ok_after_complete_write_and_stdin_is_closed() {
    with_timeout(Duration::from_secs(20), "wc -c child", || {
        let payload = utf8_payload(MIB + 3);
        let expected = payload.len();
        let mut child = Command::new("wc")
            .arg("-c")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn wc");
        let stdin = child.stdin.take().expect("piped stdin");
        let writer = spawn_stdin_writer(stdin, payload);
        // `wc` only prints after EOF, so this also proves stdin was closed.
        let output = child.wait_with_output().expect("wait wc");
        writer.finish().expect("full write must be Ok");
        let counted: usize = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .expect("wc output is a number");
        assert_eq!(counted, expected);
    });
}

#[test]
fn stdout_flood_while_writing_one_mib_stdin_does_not_deadlock_and_echoes_exactly() {
    let payload = utf8_payload(MIB);
    let expected = payload.clone();
    let (stdout, status, finished) = with_timeout(
        Duration::from_secs(30),
        "stdout flood + stdin write",
        move || {
            // The child writes 1 MiB to stdout before it starts reading stdin,
            // far more than a pipe buffer holds. A parent that wrote stdin
            // synchronously before draining stdout would deadlock here.
            let mut child = Command::new("sh")
                .args(["-c", "head -c 1048576 /dev/zero | tr '\\000' o; cat"])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn sh");
            let stdin = child.stdin.take().expect("piped stdin");
            let writer = spawn_stdin_writer(stdin, payload);
            let mut out = Vec::new();
            child
                .stdout
                .take()
                .expect("piped stdout")
                .read_to_end(&mut out)
                .expect("read stdout");
            let status = child.wait().expect("wait sh");
            (out, status, writer.finish())
        },
    );

    assert!(status.success(), "child status: {status:?}");
    finished.expect("writer must finish Ok");
    assert_eq!(stdout.len(), MIB + expected.len());
    assert!(stdout[..MIB].iter().all(|b| *b == b'o'));
    assert!(
        stdout[MIB..] == expected[..],
        "stdin must be echoed back byte-for-byte"
    );
}

// ---------------------------------------------------------------------------
// TRUNCATION_MARKER
// ---------------------------------------------------------------------------

#[test]
fn truncation_marker_has_the_documented_value() {
    assert_eq!(TRUNCATION_MARKER, "[... earlier output truncated ...]\n");
}

// ---------------------------------------------------------------------------
// Inputs that already fit
// ---------------------------------------------------------------------------

#[test]
fn inputs_that_fit_are_borrowed_unchanged() {
    let s = mixed_text();
    for max in [s.len(), s.len() + 1, usize::MAX] {
        let head = truncate_head_bytes(&s, max);
        assert!(matches!(head, Cow::Borrowed(_)), "head, max {max}");
        assert_eq!(head, s.as_str());

        let tail = truncate_tail_bytes(&s, max);
        assert!(matches!(tail, Cow::Borrowed(_)), "tail, max {max}");
        assert_eq!(tail, s.as_str());

        let marked = truncate_tail_with_marker(&s, max);
        assert!(matches!(marked, Cow::Borrowed(_)), "marker, max {max}");
        assert_eq!(marked, s.as_str());
    }
}

#[test]
fn empty_input_is_borrowed_for_every_max() {
    for max in [0, 1, 100] {
        assert!(matches!(truncate_head_bytes("", max), Cow::Borrowed("")));
        assert!(matches!(truncate_tail_bytes("", max), Cow::Borrowed("")));
        assert!(matches!(
            truncate_tail_with_marker("", max),
            Cow::Borrowed("")
        ));
    }
}

#[test]
fn max_zero_gives_empty_string() {
    let s = mixed_text();
    assert_eq!(truncate_head_bytes(&s, 0), "");
    assert_eq!(truncate_tail_bytes(&s, 0), "");
    assert_eq!(truncate_tail_with_marker(&s, 0), "");
}

// ---------------------------------------------------------------------------
// Multi-byte cut points
// ---------------------------------------------------------------------------

#[test]
fn head_cut_inside_two_byte_char_drops_the_whole_char() {
    // "a" + "é" (2 bytes) + "z"
    let s = "aéz";
    assert_eq!(truncate_head_bytes(s, 1), "a");
    assert_eq!(truncate_head_bytes(s, 2), "a");
    assert_eq!(truncate_head_bytes(s, 3), "aé");
}

#[test]
fn head_cut_inside_three_byte_char_drops_the_whole_char() {
    let s = "a€z"; // € is 3 bytes
    assert_eq!(truncate_head_bytes(s, 2), "a");
    assert_eq!(truncate_head_bytes(s, 3), "a");
    assert_eq!(truncate_head_bytes(s, 4), "a€");
}

#[test]
fn head_cut_inside_four_byte_char_drops_the_whole_char() {
    let s = "a𝄞z"; // 𝄞 is 4 bytes
    assert_eq!(truncate_head_bytes(s, 2), "a");
    assert_eq!(truncate_head_bytes(s, 3), "a");
    assert_eq!(truncate_head_bytes(s, 4), "a");
    assert_eq!(truncate_head_bytes(s, 5), "a𝄞");
}

#[test]
fn tail_cut_inside_two_byte_char_drops_the_whole_char() {
    let s = "aéz";
    assert_eq!(truncate_tail_bytes(s, 1), "z");
    assert_eq!(truncate_tail_bytes(s, 2), "z");
    assert_eq!(truncate_tail_bytes(s, 3), "éz");
}

#[test]
fn tail_cut_inside_three_byte_char_drops_the_whole_char() {
    let s = "a€z";
    assert_eq!(truncate_tail_bytes(s, 2), "z");
    assert_eq!(truncate_tail_bytes(s, 3), "z");
    assert_eq!(truncate_tail_bytes(s, 4), "€z");
}

#[test]
fn tail_cut_inside_four_byte_char_drops_the_whole_char() {
    let s = "a𝄞z";
    assert_eq!(truncate_tail_bytes(s, 2), "z");
    assert_eq!(truncate_tail_bytes(s, 3), "z");
    assert_eq!(truncate_tail_bytes(s, 4), "z");
    assert_eq!(truncate_tail_bytes(s, 5), "𝄞z");
}

#[test]
fn head_and_tail_are_bounded_maximal_and_on_char_boundaries_for_every_max() {
    let s = mixed_text();
    for max in 0..s.len() {
        let head = truncate_head_bytes(&s, max);
        assert!(head.len() <= max, "head len {} > max {max}", head.len());
        assert!(
            s.starts_with(head.as_ref()),
            "head must be a prefix, max {max}"
        );
        assert!(
            max - head.len() < 4,
            "head dropped more than one partial char at max {max}"
        );
        assert!(s.is_char_boundary(head.len()));

        let tail = truncate_tail_bytes(&s, max);
        assert!(tail.len() <= max, "tail len {} > max {max}", tail.len());
        assert!(
            s.ends_with(tail.as_ref()),
            "tail must be a suffix, max {max}"
        );
        assert!(
            max - tail.len() < 4,
            "tail dropped more than one partial char at max {max}"
        );
        assert!(s.is_char_boundary(s.len() - tail.len()));
    }
}

// ---------------------------------------------------------------------------
// truncate_tail_with_marker
// ---------------------------------------------------------------------------

#[test]
fn marker_variant_keeps_tail_behind_marker_within_max() {
    let s = mixed_text().repeat(20);
    for max in TRUNCATION_MARKER.len() + 1..s.len() {
        let kept = truncate_tail_with_marker(&s, max);
        assert!(kept.len() <= max, "len {} > max {max}", kept.len());
        let rest = kept
            .strip_prefix(TRUNCATION_MARKER)
            .unwrap_or_else(|| panic!("result must start with the marker at max {max}"));
        assert!(
            s.ends_with(rest),
            "the text after the marker must be a tail"
        );
        assert_eq!(
            rest,
            truncate_tail_bytes(&s, max - TRUNCATION_MARKER.len()),
            "the kept tail must use all space left after the marker, max {max}"
        );
    }
}

#[test]
fn marker_variant_with_max_smaller_than_marker_gives_bare_tail() {
    let s = mixed_text();
    for max in 1..TRUNCATION_MARKER.len() {
        let kept = truncate_tail_with_marker(&s, max);
        assert!(kept.len() <= max);
        assert!(
            !kept.contains("[..."),
            "no marker fragment when max {max} is smaller than the marker"
        );
        assert_eq!(kept, truncate_tail_bytes(&s, max), "bare tail at max {max}");
    }
}

#[test]
fn marker_variant_cut_inside_multibyte_chars_stays_valid() {
    for ch in ["é", "€", "𝄞"] {
        let s = ch.repeat(10_000);
        for extra in 0..4 {
            let max = TRUNCATION_MARKER.len() + 101 + extra;
            let kept = truncate_tail_with_marker(&s, max);
            assert!(kept.len() <= max);
            let rest = kept
                .strip_prefix(TRUNCATION_MARKER)
                .expect("marker present");
            assert!(rest.chars().all(|c| c.to_string() == ch));
            assert!(max - kept.len() < ch.len());
        }
    }
}

#[test]
fn documented_example_holds() {
    let log = "é".repeat(40_000); // 80,000 bytes
    let kept = truncate_tail_with_marker(&log, 32 * 1024);
    assert!(kept.len() <= 32 * 1024);
    assert!(kept.starts_with("[... earlier output truncated ...]\n"));
}

// ---------------------------------------------------------------------------
// write_stdin_async
// ---------------------------------------------------------------------------

/// Upper bound for one async write test.
const WRITE_DEADLINE: Duration = Duration::from_secs(30);

fn tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn write_stdin_async_delivers_chunks_back_to_back() {
    tokio_runtime().block_on(async {
        let mut child = tokio::process::Command::new("cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let prompt = "p".repeat(MIB);
        let chunks: [&[u8]; 3] = [b"key", b"\n", prompt.as_bytes()];
        // A hang must fail the test, not stall CI.
        let (written, output) = tokio::time::timeout(WRITE_DEADLINE, async {
            tokio::join!(write_stdin_async(stdin, &chunks), child.wait_with_output())
        })
        .await
        .expect("writing to a reading child must not hang");
        assert_eq!(written.unwrap(), StdinDelivery::Complete);
        let output = output.unwrap();
        assert_eq!(output.stdout.len(), 4 + MIB);
        assert!(output.stdout.starts_with(b"key\np"));
    });
}

#[test]
fn write_stdin_async_reports_reader_closed() {
    tokio_runtime().block_on(async {
        let mut child = tokio::process::Command::new("true")
            .stdin(Stdio::piped())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        child.wait().await.unwrap();
        let payload = vec![b'x'; 4 * MIB];
        let written = tokio::time::timeout(WRITE_DEADLINE, write_stdin_async(stdin, &[&payload]))
            .await
            .expect("writing to a child that never reads stdin must not hang")
            .unwrap();
        assert_eq!(written, StdinDelivery::ReaderClosed);
    });
}
