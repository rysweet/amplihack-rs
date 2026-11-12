//! Group (a): `parse_github_repo_identity` from the shared helper, called
//! directly. Inputs go through argv only, never a shell-interpolated literal.

use super::cases::ORIGIN_CASES;
use super::fixture::{Outcome, helper_path};
use std::fs;
use std::process::Command;

fn parse_with_lang(url: &str, lang: &str) -> Outcome {
    let output = Command::new("bash")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("LANG", lang)
        .arg("-c")
        .arg(r#". "$1"; parse_github_repo_identity "$2""#)
        .arg("parse")
        .arg(helper_path())
        .arg(url)
        .output()
        .expect("run bash");
    Outcome::from(output)
}

fn parse(url: &str) -> Outcome {
    parse_with_lang(url, "C")
}

#[test]
fn helper_exists_is_executable_and_cites_the_issues() {
    let path = helper_path();
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("{} must exist: {e}", path.display()));
    assert!(text.starts_with("#!/usr/bin/env bash\n"), "shebang");
    assert!(text.contains("#1642"), "header must cite #1642");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(&path)
            .expect("stat helper")
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "helper must be executable");
    }
}

/// Every row of the shared table (`cases.rs`, also run against the Rust twin).
#[test]
fn shared_case_table_matches_the_bash_parser() {
    let mut failures = Vec::new();
    for (url, want) in ORIGIN_CASES {
        let out = parse(url);
        let ok = match want {
            Some(want) => {
                out.success() && out.stdout == format!("{want}\n") && out.stderr.is_empty()
            }
            None => {
                out.output.status.code() == Some(1)
                    && out.stdout.is_empty()
                    && out.stderr.is_empty()
            }
        };
        if !ok {
            failures.push(format!("{url:?} -> want {want:?}\n{}", out.describe()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

#[test]
fn shared_case_table_covers_the_issue_urls_and_every_real_form() {
    use super::fixture::{HOSTILE_ORIGINS, REAL_ORIGINS};
    for url in HOSTILE_ORIGINS {
        assert!(
            ORIGIN_CASES.contains(&(*url, None)),
            "{url:?} must be a rejected row of the shared table"
        );
    }
    for url in REAL_ORIGINS {
        assert!(
            ORIGIN_CASES.contains(&(*url, Some("o/r"))),
            "{url:?} must be an o/r row of the shared table"
        );
    }
}

#[test]
fn non_ascii_owner_is_rejected_even_under_a_utf8_locale() {
    for lang in ["C.UTF-8", "en_US.UTF-8"] {
        let out = parse_with_lang("https://github.com/\u{f3}wner/r", lang);
        assert_eq!(
            out.output.status.code(),
            Some(1),
            "LANG={lang}: bracket ranges must not admit non-ASCII\n{}",
            out.describe()
        );
        assert!(out.stdout.is_empty(), "LANG={lang}: {}", out.describe());
    }
}

#[test]
fn sourcing_is_silent_and_safe_under_strict_mode() {
    let output = Command::new("bash")
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .arg("-c")
        .arg(
            r#"set -euo pipefail
. "$1"
echo sourced
if parse_github_repo_identity "$2"; then echo accepted; else echo "rejected:$?"; fi
if parse_github_repo_identity; then echo accepted-empty; else echo "rejected-empty:$?"; fi
echo "LC_ALL=${LC_ALL-unset}"
case $- in *e*) echo errexit-kept ;; esac
echo done"#,
        )
        .arg("strict")
        .arg(helper_path())
        .arg("https://evil.example#@github.com/o/r")
        .output()
        .expect("run bash");
    let out = Outcome::from(output);
    assert!(out.success(), "{}", out.describe());
    assert_eq!(
        out.stdout,
        "sourced\nrejected:1\nrejected-empty:1\nLC_ALL=unset\nerrexit-kept\ndone\n",
        "sourcing must print nothing, leak no LC_ALL and leave shell options alone\n{}",
        out.describe()
    );
    assert!(out.stderr.is_empty(), "{}", out.describe());
}
