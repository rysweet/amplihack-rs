//! Source guard for issue #1844 (SR-11, SR-12): `usage_direct.rs` holds
//! OAuth tokens, so nothing in it may format an error, print a binding that
//! holds a secret, or build a URL with a query or fragment. The rotate
//! source guard covers `rotate/` only; this one covers the direct reader.
//!
//! Contract:
//! - `usage_direct.rs` exists, and only `map_err` names `ureq::Error`.
//! - No line formats an error with `{e}`, `{e:?}`, `{err}`, `{:?}` or
//!   `.to_string()` on an error, and there is no `dbg!`.
//! - No `print!`, `eprint!`, `format!`, `panic!`, `tracing` or `log` macro
//!   names a `token`, `creds`, `doc`, `refresh` or `access` binding.
//! - No string literal used as a URL holds `?` or `#`.
//! - `Token` and `Creds` do not derive `Debug`, `Clone` or `Serialize`.

use std::fs;
use std::path::{Path, PathBuf};

fn source() -> (PathBuf, String) {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands/profile/usage_direct.rs");
    let text = fs::read_to_string(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
    (p, text)
}

/// Code lines: comments and the `#[cfg(test)]` tail are left out.
fn code_lines(text: &str) -> Vec<(usize, String)> {
    let end = text.find("#[cfg(test)]").unwrap_or(text.len());
    text[..end]
        .lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim().to_string()))
        .filter(|(_, l)| !l.starts_with("//"))
        .collect()
}

fn offending(pred: impl Fn(&str) -> bool) -> Vec<String> {
    let (_, text) = source();
    code_lines(&text)
        .into_iter()
        .filter(|(_, l)| pred(l))
        .map(|(n, l)| format!("{n}: {l}"))
        .collect()
}

#[test]
fn only_map_err_looks_at_ureq_errors() {
    let (_, text) = source();
    let lines = code_lines(&text);
    let users: Vec<_> = lines
        .iter()
        .filter(|(_, l)| l.contains("ureq::Error"))
        .collect();
    assert!(!users.is_empty(), "map_err takes a ureq::Error");
    for (n, l) in &users {
        assert!(
            l.contains("fn map_err") || l.starts_with("use ") || l.starts_with("Err(ureq::Error"),
            "ureq::Error outside map_err at line {n}: {l}"
        );
    }
}

#[test]
fn no_error_is_formatted() {
    let bad = offending(|l| {
        [
            "{e}",
            "{e:?}",
            "{err}",
            "{err:?}",
            "{error}",
            "{error:?}",
            "{:?}",
            "{:#?}",
            "dbg!(",
            "e.to_string()",
            "err.to_string()",
            "error.to_string()",
        ]
        .iter()
        .any(|p| l.contains(p))
    });
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn no_secret_binding_is_printed_or_formatted() {
    const MACROS: [&str; 10] = [
        "println!",
        "eprintln!",
        "print!",
        "eprint!",
        "format!",
        "panic!",
        "tracing::",
        "log::",
        "debug!",
        "trace!",
    ];
    const SECRETS: [&str; 6] = ["token", "creds", "doc", "refresh", "access", "bearer"];
    let bad = offending(|l| {
        MACROS.iter().any(|m| l.contains(m)) && {
            let lower = l.to_ascii_lowercase();
            // The fixed reason texts and the `Bearer ` prefix are allowed;
            // a binding inside braces is not.
            SECRETS
                .iter()
                .any(|s| lower.contains(&format!("{{{s}")) || lower.contains(&format!(", {s}")))
        }
    });
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn no_url_literal_has_a_query_or_fragment() {
    let bad = offending(|l| {
        l.split('"').skip(1).step_by(2).any(|lit| {
            (lit.starts_with("http") || lit.starts_with('/'))
                && (lit.contains('?') || lit.contains('#'))
        })
    });
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn secret_holders_do_not_derive_debug_clone_or_serialize() {
    let (_, text) = source();
    let lines: Vec<&str> = text.lines().collect();
    for name in ["struct Token", "struct Creds"] {
        let at = lines
            .iter()
            .position(|l| l.contains(name))
            .unwrap_or_else(|| panic!("{name} is defined"));
        // The attributes directly above the definition.
        let mut i = at;
        while i > 0 && {
            let above = lines[i - 1].trim_start();
            above.starts_with("#[") || above.starts_with("///")
        } {
            i -= 1;
            let l = lines[i];
            if l.trim_start().starts_with("#[derive") {
                for banned in ["Debug", "Clone", "Serialize"] {
                    assert!(!l.contains(banned), "{name} derives {banned}: {l}");
                }
            }
        }
    }
}

#[test]
fn proxies_are_not_read_from_the_environment() {
    let bad = offending(|l| l.contains("try_proxy_from_env") || l.contains("Proxy::new"));
    assert!(bad.is_empty(), "{bad:#?}");
}
