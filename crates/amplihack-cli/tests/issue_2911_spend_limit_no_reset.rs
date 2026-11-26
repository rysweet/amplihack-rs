//! Issue #2911: a spend limit that states no reset waits for the login to
//! change and retries, instead of `skipped reason=spend_limit`, with or
//! without a running rotator. The wait has no deadline; the tests change the
//! login once it is under way, as a person would.

#![cfg(unix)]

use crate::issue_1618_fixture::{Fixture, SPEND_TEXT, describe, events, sha256_hex};
use crate::issue_1618_wait_profiles::with_profiles;
use crate::issue_1801_capacity_wait::x_is_live;
use std::fs;
use std::io::Write;

#[test]
fn a_spend_limit_with_no_reset_waits_and_retries_with_no_rotator() {
    let f = with_profiles();
    x_is_live(&f);
    fs::create_dir_all(f.home.join(".claude")).unwrap();
    let mut c = f.step();
    c.env_remove("CLAUDE_CONFIG_DIR")
        .env("LIMIT_MESSAGE", SPEND_TEXT);
    let out = f.run_with_a_login_change_in(&f.home.join(".claude"), &mut c);
    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "{}", describe(&out));
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=spend_limit")),
        "{ev:?}"
    );
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped reason=spend_limit")),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|l| l.starts_with("failed")), "{ev:?}");
}

/// Issue #3036: a spend limit on a step whose input is an
/// `--input-format` channel waits and retries like any other.
///
/// This test used to pin the opposite: the channel was never copied, so the
/// step ended with the limit result and `skipped reason=piped_prompt`. It is
/// copied now as any piped prompt is. A supervised attempt's stdout is held
/// back until the step finishes, and an attempt that is run again has its
/// output discarded, so the caller has seen nothing of the attempt that met
/// the limit, and a retry given the same input is the same step from its
/// side. The caller writes its message and closes the channel; one that
/// holds it open is covered in `issue_2815_piped_prompt_wait`. The message
/// is written into an OS pipe before the step starts, which it fits in, so
/// the step reads it from a real channel.
#[test]
fn issue_3036_a_spend_limit_on_a_stream_channel_waits_and_retries_with_the_same_input() {
    let f = Fixture::new();
    let mut c = f.cmd(&[
        "claude",
        "--subprocess-safe",
        "--",
        "-p",
        "--input-format",
        "stream-json",
    ]);
    let message = b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":\"go\"}}\n";
    let (read, mut write) = std::io::pipe().expect("a pipe");
    write.write_all(message).expect("write the message");
    drop(write);
    c.env("LIMIT_MESSAGE", SPEND_TEXT)
        .env("LIMIT_CALLS", "1")
        .stdin(read);
    let out = f.run_with_a_login_change_per_open_wait(&mut c);

    assert!(out.status.success(), "{}", describe(&out));
    assert_eq!(f.count("0"), 2, "the step ran again\n{}", describe(&out));
    let want = sha256_hex(message);
    assert_eq!(f.call("0", 1)["STDIN"], want, "{}", describe(&out));
    assert_eq!(
        f.call("0", 2)["STDIN"],
        want,
        "the retry read the same input\n{}",
        describe(&out)
    );
    let ev = events(&out);
    assert!(
        ev.iter()
            .any(|l| l.starts_with("waiting_for_capacity reason=spend_limit")),
        "{ev:?}"
    );
    assert!(ev.iter().any(|l| l == "resume attempt=2"), "{ev:?}");
    assert!(
        !ev.iter()
            .any(|l| l.starts_with("skipped") || l.starts_with("failed")),
        "{ev:?}"
    );
}
