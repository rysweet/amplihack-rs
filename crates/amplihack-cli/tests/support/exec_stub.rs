//! Write executable stubs without this process holding a write descriptor.
//!
//! Exec of a file fails with ETXTBSY while any process holds it open for
//! writing. If this process wrote a stub itself, a sibling test that forks
//! during the write would inherit the descriptor and keep it until that
//! child execs, and a concurrent exec of the stub would fail. Every module
//! of `cli_integration` shares one process, so that window is real. Here a
//! short-lived `sh` writes the file instead. This process never opens it for
//! writing, so no fork of ours can inherit such a descriptor, whoever later
//! execs the stub.
//!
//! The `sh` writes a temporary name (`<path>.stub-tmp.<pid of sh>`), sets the
//! mode and renames it over `path`. A test that rewrites a stub while an
//! earlier copy is still running therefore replaces the directory entry
//! instead of opening the running file, which would fail with ETXTBSY.
//!
//! This file uses only `std`. `amplihack-cli`'s and `amplihack-hooks`'s
//! `test_support` modules and the `integration` target include it with
//! `#[path]`, so there is one stub writer.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// Writes `body` to `path` (replacing it by rename) with permission bits
/// `mode`. The path and mode reach `sh` as arguments, never as script text.
///
/// Panics before writing anything if `mode` has a setuid, setgid or sticky
/// bit (`mode & 0o7000 != 0`): a test stub never needs one.
pub fn write_executable(path: &Path, body: impl AsRef<[u8]>, mode: u32) {
    assert!(
        mode & 0o7000 == 0,
        "refusing to write stub {} with mode {mode:o}: setuid, setgid and sticky \
         bits (0o7000) are not allowed",
        path.display()
    );
    let mut child = Command::new("/bin/sh")
        .args([
            "-c",
            "t=\"$1.stub-tmp.$$\"; cat > \"$t\" && chmod \"$2\" \"$t\" && mv -f \"$t\" \"$1\"",
            "sh",
        ])
        .arg(path)
        .arg(format!("{mode:o}"))
        // `cat`, `chmod` and `mv` are resolved through the child's PATH.
        // Sibling tests in this process set PATH to a directory of their own
        // (an empty one, or a fixture bin) while they run, and the variable
        // is process-wide, so an inherited PATH makes this `sh` exit 127 at
        // random. The three are in `/usr/bin` or `/bin` on every platform
        // these tests run on.
        .env("PATH", "/usr/bin:/bin")
        .stdin(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| panic!("cannot start sh to write {}: {e}", path.display()));
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(body.as_ref())
        .unwrap_or_else(|e| panic!("cannot stream {}: {e}", path.display()));
    let status = child.wait().expect("wait for sh");
    assert!(
        status.success(),
        "writing {} failed: {status}",
        path.display()
    );
}

/// Copies `from` to `to`, mode bits included, through a `cp` child, for the
/// same reason as `write_executable`: `fs::copy` would open `to` for writing
/// in this process.
pub fn copy_executable(from: &Path, to: &Path) {
    let status = Command::new("/bin/cp")
        .arg("--")
        .arg(from)
        .arg(to)
        .status()
        .unwrap_or_else(|e| panic!("cannot start cp for {}: {e}", to.display()));
    assert!(
        status.success(),
        "copying {} to {} failed: {status}",
        from.display(),
        to.display()
    );
}

/// `value` as one word of POSIX shell text: wrapped in single quotes, with
/// each embedded single quote written as `'\''`. Nothing inside single
/// quotes is expanded, so the shell hands the value on byte for byte.
///
/// Every value a test writes into the text of a stub goes through this (a
/// folder, a FIFO, the test executable's path): a temporary path may hold a
/// space or a quote, and a stub must never run part of a path as code. The
/// value must come from the test, never from a child's output.
#[allow(dead_code)]
pub fn sh_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('\'');
    for c in value.chars() {
        if c == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(c);
        }
    }
    quoted.push('\'');
    quoted
}

#[cfg(test)]
mod sh_quote_tests {
    use super::sh_quote;
    use std::process::Command;

    /// What `sh` makes of `sh_quote(value)` when it is the one argument of
    /// `printf %s`: the value must come back byte for byte.
    fn through_sh(value: &str) -> String {
        let script = format!("printf %s {}", sh_quote(value));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .output()
            .expect("start sh");
        assert!(out.status.success(), "{script}: {out:?}");
        String::from_utf8(out.stdout).expect("utf-8 out")
    }

    #[test]
    fn a_value_with_shell_syntax_reaches_the_script_unchanged() {
        for value in [
            "plain",
            "",
            "it's here",
            "'",
            "two'quotes'in",
            "has a space",
            "$HOME and ${HOME} and $(id)",
            "`id`",
            "a;b&c|d>e<f",
            "back\\slash",
            "new\nline",
            "/tmp/it's a $dir/with `ticks`",
        ] {
            assert_eq!(through_sh(value), value, "value {value:?}");
        }
    }

    #[test]
    fn the_quoted_form_is_one_word() {
        // A value that would split or expand if left bare stays one word.
        let script = format!("set -- {}; printf %s \"$#\"", sh_quote("a b  c $x"));
        let out = Command::new("/bin/sh")
            .args(["-c", &script])
            .env_clear()
            .output()
            .expect("start sh");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "1");
    }
}
