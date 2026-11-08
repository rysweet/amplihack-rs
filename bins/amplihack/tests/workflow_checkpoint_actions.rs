//! Per-action behaviour of `amplihack internal workflow-checkpoint` (#1554):
//! the pure formatters, `worktree-path`, `sha256`, and the advertisement
//! parser reached through `advertised-ref` with a `git` shim.
//!
//! Expected bytes for the formatters are what the Python at 918c256cf printed
//! (`json.dumps(dict(...))`: default ', ' and ': ' separators, insertion
//! order, ensure_ascii).

use std::io::Write;
use std::process::Stdio;

use crate::workflow_checkpoint_support::*;

// ---------------------------------------------------------------- formatters

fn admission(sb: &Sandbox, env: Option<&str>) -> std::process::Output {
    let mut c = sb.checkpoint("admission-refused-result", [""; 0]);
    if let Some(v) = env {
        c.env("RECIPE_VAR_worktree_setup", v);
    }
    c.output().unwrap()
}

fn admission_line(branch_json: &str) -> Vec<u8> {
    format!(
        "{{\"pushed\": \"false\", \"sha\": \"\", \"branch\": {branch_json}, \"reason\": \"scope-admission-refused\"}}\n"
    )
    .into_bytes()
}

#[test]
fn admission_refused_result_prints_the_branch_in_python_key_order() {
    let sb = Sandbox::new();
    let out = admission(
        &sb,
        Some(r#"{"worktree_path":"/w","branch_name":"feat/x"}"#),
    );
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(out.stdout, admission_line("\"feat/x\""));
}

#[test]
fn admission_refused_result_escapes_non_ascii_like_ensure_ascii() {
    let sb = Sandbox::new();
    let out = admission(&sb, Some("{\"branch_name\":\"feat/caf\u{e9}-\u{1F600}\"}"));
    assert_eq!(status(&out), 0);
    assert_eq!(
        out.stdout,
        admission_line("\"feat/caf\\u00e9-\\ud83d\\ude00\"")
    );
}

#[test]
fn cleanup_skip_result_is_byte_identical_to_the_python() {
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("cleanup-skip-result", ["worktree path gone"])
        .output()
        .unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"{\"skipped\": true, \"admitted\": false, \"reason\": \"worktree path gone\"}\n"
    );
}

#[test]
fn cleanup_skip_result_escapes_quotes_controls_and_non_ascii() {
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("cleanup-skip-result", ["a\"b\\c\td\u{e9}"])
        .output()
        .unwrap();
    assert_eq!(status(&out), 0);
    assert_eq!(
        out.stdout,
        b"{\"skipped\": true, \"admitted\": false, \"reason\": \"a\\\"b\\\\c\\td\\u00e9\"}\n"
    );
}

// --------------------------------------------------------------------- sha256

fn sha256_of(sb: &Sandbox, input: &[u8]) -> std::process::Output {
    let mut child = sb
        .checkpoint("sha256", [""; 0])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn sha256_prints_lowercase_hex_and_lf() {
    let sb = Sandbox::new();
    let out = sha256_of(&sb, b"abc");
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(
        out.stdout,
        b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\n"
    );
}

#[test]
fn sha256_of_empty_and_binary_input() {
    let sb = Sandbox::new();
    assert_eq!(
        sha256_of(&sb, b"").stdout,
        b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n"
    );
    let out = sha256_of(&sb, b"\x00\xff\xfe\n");
    assert_eq!(status(&out), 0);
    assert_eq!(out.stdout.len(), 65);
}

#[test]
fn sha256_ignores_arguments_like_the_inline_python() {
    // The 918c one-liner never read sys.argv.
    let sb = Sandbox::new();
    let out = sb.checkpoint("sha256", ["file"]).output().unwrap();
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(out.stdout.len(), 65);
}

// --------------------------------------------------------------- worktree-path

fn worktree_path_env(sb: &Sandbox, value: &str) -> std::process::Output {
    sb.checkpoint("worktree-path", [""; 0])
        .env("RECIPE_VAR_worktree_setup", value)
        .output()
        .unwrap()
}

#[test]
fn worktree_path_prints_raw_bytes_and_lf_from_env() {
    let sb = Sandbox::new();
    let out = worktree_path_env(&sb, r#"{"worktree_path":"/w/feat x","branch_name":"b"}"#);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert_eq!(out.stdout, b"/w/feat x\n");
}

#[test]
fn worktree_path_decodes_escapes_to_utf8_bytes() {
    let sb = Sandbox::new();
    let out = worktree_path_env(&sb, r#"{"worktree_path":"/w/caf\u00e9"}"#);
    assert_eq!(status(&out), 0);
    assert_eq!(out.stdout, "/w/caf\u{e9}\n".as_bytes());
}

#[test]
fn worktree_path_never_reads_stdin() {
    // os.environ["RECIPE_VAR_worktree_setup"] raised KeyError when the
    // variable was unset (status 1); stdin was never read.
    let sb = Sandbox::new();
    let mut child = sb
        .checkpoint("worktree-path", [""; 0])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"{"worktree_path":"/from/stdin"}"#);
    let out = child.wait_with_output().unwrap();
    assert_eq!(status(&out), 1, "stderr: {}", stderr(&out));
    assert!(out.stdout.is_empty());
}

#[test]
fn worktree_path_never_reads_argv() {
    // The recipe one-liner read only the environment; argv is ignored.
    let sb = Sandbox::new();
    let out = sb
        .checkpoint("worktree-path", [r#"{"worktree_path":"/argv"}"#])
        .env("RECIPE_VAR_worktree_setup", r#"{"worktree_path":"/env"}"#)
        .output()
        .unwrap();
    assert_eq!((status(&out), out.stdout.as_slice()), (0, &b"/env\n"[..]));
}

// ------------------------------------------------- advertised-ref and parser

/// Run advertised-ref on saved `ls-remote` output, the 918c argv order:
/// `<saved-output-file> <ref> <ls-remote-rc>`.
fn advertised(body: &[u8], ls_status: i32, target: &str) -> std::process::Output {
    let sb = Sandbox::new();
    let saved = sb.tmp().join("saved-advertisement");
    std::fs::write(&saved, body).unwrap();
    sb.checkpoint(
        "advertised-ref",
        [
            saved.as_os_str(),
            target.as_ref(),
            ls_status.to_string().as_ref(),
        ],
    )
    .output()
    .unwrap()
}

fn line(oid: &str, name: &str) -> String {
    format!("{oid}\t{name}\n")
}

const TARGET: &str = "refs/heads/feat/x";

#[test]
fn advert_present_ref_exits_0() {
    let body = line(&oid1('a'), "HEAD") + &line(&oid1('a'), TARGET);
    let out = advertised(body.as_bytes(), 0, TARGET);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    assert!(
        out.stdout.is_empty() && out.stderr.is_empty(),
        "advertised-ref reports by status only"
    );
}

#[test]
fn advert_absent_ref_from_a_valid_advertisement_exits_3() {
    let body = line(&oid1('a'), "refs/heads/main");
    let out = advertised(body.as_bytes(), 0, TARGET);
    assert_eq!(status(&out), 3, "stderr: {}", stderr(&out));
}

#[test]
fn advert_valid_empty_advertisement_with_rc_2_is_absent() {
    let out = advertised(b"", 2, TARGET);
    assert_eq!(status(&out), 3, "stderr: {}", stderr(&out));
}

#[test]
fn advert_rc_2_with_refs_is_malformed() {
    let body = line(&oid1('a'), "refs/heads/main");
    let out = advertised(body.as_bytes(), 2, TARGET);
    assert_eq!(status(&out), 78);
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn advert_peeled_tag_lines_and_head_are_accepted() {
    let body = line(&oid1('a'), "HEAD")
        + &line(&oid1('b'), "refs/tags/v1")
        + &line(&oid1('c'), "refs/tags/v1^{}")
        + &line(&oid1('d'), TARGET);
    let out = advertised(body.as_bytes(), 0, TARGET);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
}

#[test]
fn advert_matches_the_exact_ref_bytes_only() {
    // Prefix, suffix, case-only and other-remote names never count as present.
    for other in [
        "refs/heads/feat",
        "refs/heads/feat/x2",
        "refs/heads/feat/xy",
        "refs/heads/Feat/X",
        "refs/remotes/upstream/feat/x",
        "refs/heads/feat/x^{}",
        "refs/tags/feat/x",
    ] {
        let body = line(&oid1('a'), other);
        let out = advertised(body.as_bytes(), 0, TARGET);
        assert_eq!(status(&out), 3, "{other} matched {TARGET}");
    }
}

/// Real `git ls-remote --exit-code -- origin` output and status, as the
/// shell saves them.
fn real_ls_remote(sb: &Sandbox, work: &std::path::Path) -> (Vec<u8>, i32) {
    let out = sb
        .command("git")
        .args(["ls-remote", "--exit-code", "--", "origin"])
        .current_dir(work)
        .output()
        .unwrap();
    (out.stdout, out.status.code().unwrap())
}

#[test]
fn advert_accepts_real_ls_remote_output_with_annotated_tags() {
    // Real git: a remote with an annotated tag advertises a peeled line.
    let sb = Sandbox::new();
    let (work, _bare) = repo_with_origin(&sb);
    git(&sb, &work, &["tag", "-a", "v1", "-m", "annotated"]);
    git(&sb, &work, &["push", "-q", "origin", "v1", "feat/x"]);
    let (body, rc) = real_ls_remote(&sb, &work);
    assert!(String::from_utf8_lossy(&body).contains("refs/tags/v1^{}"));
    let out = advertised(&body, rc, TARGET);
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    let out = advertised(&body, rc, "refs/heads/never-pushed");
    assert_eq!(status(&out), 3, "stderr: {}", stderr(&out));
}

#[test]
fn advert_accepts_either_object_id_width() {
    // ORIGIN_REF_PY checked each ID against 40 or 64 digits; it never
    // compared the width with the repository's object format.
    for oid in [oid256('a'), oid1('a')] {
        let out = advertised(line(&oid, TARGET).as_bytes(), 0, TARGET);
        assert_eq!(status(&out), 0, "{}: {}", oid.len(), stderr(&out));
    }
}

#[test]
fn advert_target_with_non_utf8_bytes_is_compared_as_bytes() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    let sb = Sandbox::new();
    let mut body = oid1('a').into_bytes();
    body.extend_from_slice(b"\trefs/heads/caf\xe9\n");
    let saved = sb.tmp().join("saved");
    std::fs::write(&saved, &body).unwrap();
    let run = |target: Vec<u8>| {
        sb.checkpoint(
            "advertised-ref",
            [
                saved.clone().into_os_string(),
                OsString::from_vec(target),
                OsString::from("0"),
            ],
        )
        .output()
        .unwrap()
    };
    let out = run(b"refs/heads/caf\xe9".to_vec());
    assert_eq!(status(&out), 0, "stderr: {}", stderr(&out));
    let out = run(b"refs/heads/caf\xe8".to_vec());
    assert_eq!(status(&out), 3, "distinct non-UTF-8 refs collided");
}
