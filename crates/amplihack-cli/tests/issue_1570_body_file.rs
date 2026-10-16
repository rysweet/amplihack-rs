//! Issue #1570 — `amplifier-bundle/tools/workflow_body_file.sh`.
//!
//! Agent-written issue and PR bodies used to reach `gh` as `--body "$BODY"`,
//! one argv element, so a body over 131,072 bytes failed the spawn with
//! `Argument list too long (os error 7)`. The sites now write the body to a
//! file with `write_body_file <OUT> [<SOURCE>] < input` and pass
//! `--body-file`. The helper keeps the body under GitHub's 65,536-character
//! limit by truncating at `WORKFLOW_BODY_MAX_BYTES` (default 60000) on a UTF-8
//! boundary and appending a fixed note.
//!
//! Contract: docs/reference/large-payload-delivery.md, section
//! "Shell helper: workflow_body_file.sh". These tests execute the real script,
//! both sourced and run directly, and compare its output byte for byte.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const DEFAULT_MAX: usize = 60_000;
const DEFAULT_SOURCE: &str = "the workflow step output";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root is two levels above the crate manifest")
        .to_path_buf()
}

fn script() -> PathBuf {
    let path = repo_root().join("amplifier-bundle/tools/workflow_body_file.sh");
    assert!(
        path.is_file(),
        "issue #1570: {} must exist (shared helper for --body-file and git commit -F)",
        path.display()
    );
    path
}

#[derive(Clone, Copy, Debug)]
enum Mode {
    Sourced,
    Direct,
}

const MODES: [Mode; 2] = [Mode::Sourced, Mode::Direct];

/// Run the helper with a scrubbed environment. `max` sets
/// `WORKFLOW_BODY_MAX_BYTES` when given.
fn run(mode: Mode, cwd: &Path, args: &[&str], max: Option<usize>, input: &[u8]) -> Output {
    let script = script();
    let mut cmd = Command::new("bash");
    match mode {
        // The caller runs under `set -euo pipefail`, as recipe steps do, and
        // must keep running after the function returns.
        Mode::Sourced => {
            cmd.arg("-c")
                .arg(
                    "set -euo pipefail; umask 022; . \"$1\"; shift; \
                     write_body_file \"$@\"; echo SOURCED_CALLER_CONTINUES",
                )
                .arg("bash")
                .arg(&script);
        }
        Mode::Direct => {
            cmd.arg("-c")
                .arg("umask 022; exec bash \"$@\"")
                .arg("bash")
                .arg(&script);
        }
    }
    cmd.args(args);
    let tmpdir = cwd.join("private-tmp");
    std::fs::create_dir_all(&tmpdir).unwrap();
    cmd.env_clear()
        .env(
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        )
        .env("HOME", cwd)
        .env("TMPDIR", &tmpdir)
        .env("LC_ALL", "C")
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(m) = max {
        cmd.env("WORKFLOW_BODY_MAX_BYTES", m.to_string());
    }
    let mut child = cmd.spawn().expect("spawn bash");
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        // The helper may exit early on usage errors; a broken pipe is fine.
        let _ = stdin.write_all(input);
    }
    child.wait_with_output().expect("wait bash")
}

fn expected(input: &[u8], max: usize, source: &str) -> Vec<u8> {
    if input.len() <= max {
        return input.to_vec();
    }
    let s = std::str::from_utf8(input).expect("test input is UTF-8");
    let mut keep = max;
    while !s.is_char_boundary(keep) {
        keep -= 1;
    }
    let mut out = input[..keep].to_vec();
    out.extend_from_slice(
        format!(
            "\n\n[... truncated {} bytes; full text in {source} ...]",
            input.len() - keep
        )
        .as_bytes(),
    );
    out
}

fn assert_ok(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{ctx}: helper failed with {:?}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn assert_caller_continues(mode: Mode, out: &Output) {
    if let Mode::Sourced = mode {
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("SOURCED_CALLER_CONTINUES"),
            "sourced write_body_file must return to its caller, not exit it"
        );
    }
}

fn large_input(len: usize) -> Vec<u8> {
    let unit = "héllo wörld € 😀 \"quoted\" $HOME `x` line\n";
    let mut s = String::new();
    while s.len() < len {
        s.push_str(unit);
    }
    s.into_bytes()
}

#[test]
fn small_input_is_written_byte_for_byte() {
    let inputs: [&[u8]; 4] = [
        b"",
        b"single line without newline",
        "multi\nline\n\nbody with é € 😀\n".as_bytes(),
        b"trailing newlines\n\n\n",
    ];
    for mode in MODES {
        for (i, input) in inputs.iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let out_path = dir.path().join("body.md");
            let out = run(mode, dir.path(), &[out_path.to_str().unwrap()], None, input);
            assert_ok(&out, &format!("{mode:?} input #{i}"));
            assert_caller_continues(mode, &out);
            let got = std::fs::read(&out_path).expect("OUT written");
            assert_eq!(got, *input, "{mode:?} input #{i} must be unchanged");
        }
    }
}

#[test]
fn input_of_exactly_max_bytes_is_not_truncated_and_one_more_is() {
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let exact = vec![b'a'; DEFAULT_MAX];
        let out_path = dir.path().join("exact.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap()],
            None,
            &exact,
        );
        assert_ok(&out, "exact");
        assert_eq!(std::fs::read(&out_path).unwrap(), exact);

        let over = vec![b'a'; DEFAULT_MAX + 1];
        let out_path = dir.path().join("over.md");
        let out = run(mode, dir.path(), &[out_path.to_str().unwrap()], None, &over);
        assert_ok(&out, "over");
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            expected(&over, DEFAULT_MAX, DEFAULT_SOURCE)
        );
    }
}

#[test]
fn input_over_128_kib_is_truncated_with_default_note() {
    let input = large_input(200 * 1024);
    assert!(input.len() > 131_072);
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("body.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap()],
            None,
            &input,
        );
        assert_ok(&out, &format!("{mode:?} 200 KiB"));
        assert_caller_continues(mode, &out);
        let got = std::fs::read(&out_path).unwrap();
        let text = std::str::from_utf8(&got).expect("output must be valid UTF-8");
        assert!(
            text.chars().count() <= 65_536,
            "output must fit GitHub's 65,536-character body limit"
        );
        assert_eq!(got, expected(&input, DEFAULT_MAX, DEFAULT_SOURCE));
    }
}

#[test]
fn truncation_note_names_the_given_source() {
    let input = large_input(150 * 1024);
    let source = "the publish step log";
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("body.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap(), source],
            None,
            &input,
        );
        assert_ok(&out, "source");
        assert_eq!(
            std::fs::read(&out_path).unwrap(),
            expected(&input, DEFAULT_MAX, source)
        );
    }
}

#[test]
fn cut_points_inside_multibyte_chars_back_up_to_a_boundary() {
    // Prefix "abc" (3 bytes), then the multibyte char, then a tail so the
    // input is always longer than the limit.
    let cases: &[(&str, &[usize])] = &[
        ("é", &[4, 5]),        // 2-byte: cut inside at 4, boundary at 5
        ("€", &[4, 5, 6]),     // 3-byte: inside at 4 and 5, boundary at 6
        ("😀", &[4, 5, 6, 7]), // 4-byte: inside at 4, 5, 6, boundary at 7
    ];
    for mode in MODES {
        for (ch, limits) in cases {
            let input = format!("abc{ch}tail-after-the-char").into_bytes();
            for &max in *limits {
                let dir = tempfile::tempdir().unwrap();
                let out_path = dir.path().join("body.md");
                let out = run(
                    mode,
                    dir.path(),
                    &[out_path.to_str().unwrap()],
                    Some(max),
                    &input,
                );
                assert_ok(&out, &format!("{mode:?} {ch} max={max}"));
                let got = std::fs::read(&out_path).unwrap();
                assert!(
                    std::str::from_utf8(&got).is_ok(),
                    "{mode:?} {ch} max={max}: output must be valid UTF-8, got {got:?}"
                );
                assert_eq!(
                    got,
                    expected(&input, max, DEFAULT_SOURCE),
                    "{mode:?} {ch} max={max}"
                );
            }
        }
    }
}

#[test]
fn output_is_mode_0600_and_no_temp_files_are_left() {
    let input = large_input(140 * 1024);
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_dir = dir.path().join("out");
        std::fs::create_dir(&out_dir).unwrap();
        let out_path = out_dir.join("body.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap()],
            None,
            &input,
        );
        assert_ok(&out, "mode");
        let perm = std::fs::metadata(&out_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(perm, 0o600, "{mode:?}: OUT must be 0600, got {perm:o}");
        let left: Vec<_> = std::fs::read_dir(&out_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 1, "{mode:?}: stray files beside OUT: {left:?}");
        let tmp_left: Vec<_> = std::fs::read_dir(dir.path().join("private-tmp"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            tmp_left.is_empty(),
            "{mode:?}: temp files left: {tmp_left:?}"
        );
    }
}

#[test]
fn missing_out_argument_exits_2() {
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out = run(mode, dir.path(), &[], None, b"body");
        assert_eq!(
            out.status.code(),
            Some(2),
            "{mode:?}: a missing <OUT> is a usage error (exit 2); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn unwritable_out_exits_1() {
    for mode in MODES {
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("no-such-dir").join("body.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap()],
            None,
            b"body",
        );
        assert_eq!(
            out.status.code(),
            Some(1),
            "{mode:?}: an unwritable <OUT> is a write failure (exit 1); stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(!out_path.exists());
    }
}

#[test]
fn hostile_text_is_preserved_literally_and_never_executed() {
    let hostile = "$(touch pwned-subst)\n`touch pwned-backtick`\n%s %n %d %%\n\
                   -n -e --body-file\n\\n\\t\\0 ${HOME} $((1+1))\n"
        .to_string();
    let source = "-%s $(touch pwned-source) `touch pwned-source2`";
    for mode in MODES {
        // Untruncated: byte-for-byte.
        let dir = tempfile::tempdir().unwrap();
        let out_path = dir.path().join("body.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path.to_str().unwrap(), source],
            None,
            hostile.as_bytes(),
        );
        assert_ok(&out, "hostile small");
        assert_eq!(std::fs::read(&out_path).unwrap(), hostile.as_bytes());

        // Truncated: the note carries the hostile SOURCE literally.
        let out_path2 = dir.path().join("body2.md");
        let out = run(
            mode,
            dir.path(),
            &[out_path2.to_str().unwrap(), source],
            Some(10),
            hostile.as_bytes(),
        );
        assert_ok(&out, "hostile truncated");
        assert_eq!(
            std::fs::read(&out_path2).unwrap(),
            expected(hostile.as_bytes(), 10, source)
        );

        for name in [
            "pwned-subst",
            "pwned-backtick",
            "pwned-source",
            "pwned-source2",
        ] {
            assert!(
                !dir.path().join(name).exists(),
                "{mode:?}: input or SOURCE was executed ({name} created)"
            );
        }
    }
}

fn code_lines(text: &str) -> String {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn script_has_no_eval_no_python_and_uses_private_temp_files() {
    let text = std::fs::read_to_string(script()).unwrap();
    let code = code_lines(&text);
    let eval = regex::Regex::new(r"\beval\b").unwrap();
    assert!(
        !eval.is_match(&code),
        "workflow_body_file.sh must not use eval"
    );
    assert!(
        !code.contains("python"),
        "workflow_body_file.sh must need only POSIX tools, not Python"
    );
    assert!(code.contains("umask 077"), "must run under umask 077");
    assert!(code.contains("mktemp"), "must write through a mktemp file");
    assert!(code.contains("trap"), "must clean up with a trap");
    assert!(
        code.contains("printf '%s'"),
        "the truncation note must be written with printf '%s'"
    );
}
