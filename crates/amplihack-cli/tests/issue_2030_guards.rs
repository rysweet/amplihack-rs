//! Issue #2030 (red phase): source guards that fail CI, rather than warn,
//! when a change puts prompt or contract text back on a command line.
//!
//! Read-only scans of the workspace. Allowlists are explicit and small; a new
//! entry needs a reason next to it.

use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
            return cur;
        }
        assert!(cur.pop(), "no workspace root above the crate");
    }
}

fn walk(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let ft = e.file_type().unwrap();
        if ft.is_dir() {
            let name = p.file_name().unwrap().to_string_lossy();
            if name == "target" || name.starts_with('.') {
                continue;
            }
            walk(&p, ext, out);
        } else if ft.is_file() && p.extension().is_some_and(|x| x == ext) {
            out.push(p);
        }
    }
}

/// Non-test Rust sources under `crates/*/src`.
fn production_sources() -> Vec<(String, String)> {
    let root = workspace_root();
    let mut files = Vec::new();
    for krate in fs::read_dir(root.join("crates")).unwrap().flatten() {
        walk(&krate.path().join("src"), "rs", &mut files);
    }
    files
        .into_iter()
        .filter(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            let s = p.to_string_lossy();
            !(name.starts_with("tests_")
                || name.starts_with("test_")
                || name.ends_with("_tests.rs")
                || name.ends_with("_test.rs")
                || name == "tests.rs"
                || s.contains("/tests/")
                || s.contains("/test_support"))
        })
        .map(|p| {
            let rel = p
                .strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let text = fs::read_to_string(&p).unwrap();
            // Inline `#[cfg(test)]` modules sit at the end of a file by
            // convention; scan only what comes before the first one.
            let prod = match text.find("#[cfg(test)]") {
                Some(i) => text[..i].to_string(),
                None => text,
            };
            (rel, prod)
        })
        .collect()
}

fn code_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines().enumerate().filter(|(_, l)| {
        let t = l.trim_start();
        !(t.starts_with("//") || t.starts_with("///") || t.starts_with("//!"))
    })
}

#[test]
fn no_selector_answers_argv() {
    // The old `select_mode` answered Argv when nothing safe fitted, so doctor
    // showed a channel spawns refused. It is gone: every caller, doctor
    // included, uses `try_select_mode`, which never answers Argv.
    let mut hits = Vec::new();
    for (rel, text) in production_sources() {
        for (n, line) in code_lines(&text) {
            let mut rest = line;
            while let Some(i) = rest.find("select_mode(") {
                if !rest[..i].ends_with("try_") {
                    hits.push(format!("{rel}:{}: {}", n + 1, line.trim()));
                }
                rest = &rest[i + "select_mode(".len()..];
            }
        }
    }
    assert!(
        hits.is_empty(),
        "use try_select_mode; select_mode is removed:\n{}",
        hits.join("\n")
    );
}

#[test]
fn prompt_delivery_deliver_uses_try_select_mode() {
    let src =
        fs::read_to_string(workspace_root().join("crates/amplihack-utils/src/prompt_delivery.rs"))
            .unwrap();
    assert!(src.contains("pub fn try_select_mode("));
    assert!(
        !src.contains("AUTO_TEMPFILE_THRESHOLD_BYTES"),
        "the 4 KiB argv threshold is removed"
    );
    let deliver = &src[src.find("pub fn deliver(").expect("deliver exists")..];
    let body = &deliver[..deliver.find("\n}\n").unwrap_or(deliver.len())];
    assert!(
        body.contains("try_select_mode("),
        "deliver uses the fallible selector"
    );
}

#[test]
fn no_argv_append_text_in_crates() {
    // Emitting `--append-system-prompt` (or `--system-prompt`) followed by
    // text is the #2030 incident. Only the `-file` forms may be built.
    const EMITTERS: &[&str] = &[
        "arg(\"--append-system-prompt\")",
        "args([\"--append-system-prompt\"",
        "push(\"--append-system-prompt\"",
        "\"--append-system-prompt\".to_string()",
        "\"--append-system-prompt\".into()",
        "String::from(\"--append-system-prompt\")",
        "arg(\"--system-prompt\")",
        "push(\"--system-prompt\"",
        "\"--system-prompt\".to_string()",
        "\"--system-prompt\".into()",
    ];
    const ALLOWED: &[&str] = &[
        // Detects the user's own flags and rewrites them to the -file forms.
        "crates/amplihack-cli/src/commands/launch/passthrough_rewrite.rs",
    ];
    let mut hits = Vec::new();
    for (rel, text) in production_sources() {
        if ALLOWED.contains(&rel.as_str()) {
            continue;
        }
        for (n, line) in code_lines(&text) {
            if EMITTERS.iter().any(|e| line.contains(e)) {
                hits.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "prompt text flags must not be emitted; use the -file form:\n{}",
        hits.join("\n")
    );
}

#[test]
fn launch_command_emits_the_file_flag() {
    let src = fs::read_to_string(
        workspace_root().join("crates/amplihack-cli/src/commands/launch/command.rs"),
    )
    .unwrap();
    assert!(src.contains("\"--append-system-prompt-file\""));
    assert!(
        !src.contains("installed_fragment()"),
        "command.rs passes a path, not the text"
    );
}

#[test]
fn recipes_do_not_put_prompt_on_argv() {
    let root = workspace_root();
    let mut files = Vec::new();
    walk(&root.join("amplifier-bundle"), "yaml", &mut files);
    walk(&root.join("amplifier-bundle"), "yml", &mut files);
    let mut hits = Vec::new();
    for p in files {
        let text = fs::read_to_string(&p).unwrap();
        for (n, line) in text.lines().enumerate() {
            for bad in [
                "copilot -p \"$",
                "claude -p \"$",
                "copilot --prompt \"$",
                "claude --print \"$",
            ] {
                if line.contains(bad) {
                    hits.push(format!(
                        "{}:{}: {}",
                        p.strip_prefix(&root).unwrap().display(),
                        n + 1,
                        line.trim()
                    ));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "recipe puts a prompt variable on argv:\n{}",
        hits.join("\n")
    );
}

#[test]
fn no_bundle_file_sets_prompt_delivery() {
    // The delivery mode is inherited by every nested step; no shipped recipe
    // or config may set it (and `argv` is no longer an accepted value, R1.2).
    let root = workspace_root();
    let mut files = Vec::new();
    for ext in ["yaml", "yml", "sh", "json", "toml", "md"] {
        walk(&root.join("amplifier-bundle/recipes"), ext, &mut files);
    }
    let hits: Vec<String> = files
        .iter()
        .filter(|p| {
            fs::read_to_string(p)
                .unwrap_or_default()
                .contains("AMPLIHACK_PROMPT_DELIVERY")
        })
        .map(|p| p.display().to_string())
        .collect();
    assert!(hits.is_empty(), "{hits:?}");
}

#[test]
fn quality_loop_copilot_sites_use_a_private_prompt_dir_and_trap() {
    let text =
        fs::read_to_string(workspace_root().join("amplifier-bundle/recipes/quality-loop.yaml"))
            .unwrap();
    let sites = text.matches("amplihack copilot -p").count();
    assert_eq!(sites, 3, "the three Copilot steps");
    assert_eq!(
        text.matches("mktemp -d").count(),
        3,
        "one private dir per step"
    );
    assert!(
        text.matches("trap ").count() >= 3,
        "each step removes its dir"
    );
    assert!(
        text.contains("printf '%s' \"$PROMPT\" >"),
        "write with printf, never echo"
    );
}

/// R1.2: `AMPLIHACK_PROMPT_DELIVERY=argv` is not accepted anywhere. No
/// production source parses `"argv"` into `PromptDelivery::Argv`, and no
/// user-facing text offers `argv` as a value or as a way out of
/// `NoSafeChannel`.
#[test]
fn argv_is_not_an_accepted_prompt_delivery_value() {
    let mut hits = Vec::new();
    for (rel, text) in production_sources() {
        for (n, line) in code_lines(&text) {
            let t = line.trim();
            if t.contains("\"argv\" =>") && t.contains("PromptDelivery::Argv") {
                hits.push(format!("{rel}:{}: {t}", n + 1));
            }
            for offer in [
                "AMPLIHACK_PROMPT_DELIVERY=argv",
                "auto|argv|",
                "auto, argv,",
            ] {
                if t.contains(offer) {
                    hits.push(format!("{rel}:{}: offers {offer:?}: {t}", n + 1));
                }
            }
        }
    }
    assert!(
        hits.is_empty(),
        "argv must not be an accepted or advertised delivery value:\n{}",
        hits.join("\n")
    );
}

/// R1.2: `deliver` must not build an argv delivery; the only `cmd.arg(prompt)`
/// style append was the `DeliveryMode::Argv` arm.
#[test]
fn deliver_has_no_argv_arm_that_appends_the_prompt() {
    let src =
        fs::read_to_string(workspace_root().join("crates/amplihack-utils/src/prompt_delivery.rs"))
            .unwrap();
    let deliver = &src[src.find("pub fn deliver(").expect("deliver exists")..];
    let body = &deliver[..deliver.find("\n}\n").unwrap_or(deliver.len())];
    if let Some(i) = body.find("DeliveryMode::Argv =>") {
        let arm = &body[i..];
        let arm = &arm[..arm.find("DeliveryMode::Tempfile").unwrap_or(arm.len())];
        assert!(
            !arm.contains(".arg("),
            "the Argv arm of deliver must not append to argv:\n{arm}"
        );
    }
}
