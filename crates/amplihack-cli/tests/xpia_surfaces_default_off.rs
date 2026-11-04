//! Issue #1878 item 1: the static surfaces that could turn XPIA on by
//! default, or tell a user it is on, say and do "off unless
//! `XPIA_ENABLED=true`".
//!
//! Covers the Claude Code plugin manifest and hooks (R6), the xpia-defense
//! agent and the `/xpia` command (R7, D8, D9), the docs grep (R8, D10) and
//! the removal of the `install_with_xpia.sh` scripts (D12, #1920). Reads
//! repository files only.

use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn read(rel: &str) -> String {
    fs::read_to_string(repo().join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

/// Words that describe XPIA as running by default (D10).
const DEFAULT_ON_WORDS: &[&str] = &[
    "on by default",
    "enabled by default",
    "automatic",
    "automatically",
    "transparent",
    "always on",
    "always-on",
];

/// Words that make a line correct even when it also holds a flagged word.
const OPT_IN_WORDS: &[&str] = &["off by default", "opt in", "opt-in", "xpia_enabled=true"];

/// Files the docs grep skips, each with its reason. Changelog and history
/// files only: they record what was true then.
const ALLOWLIST: &[(&str, &str)] = &[];

fn flags(line: &str) -> bool {
    let lower = line.to_lowercase();
    DEFAULT_ON_WORDS.iter().any(|w| lower.contains(w))
        && !OPT_IN_WORDS.iter().any(|w| lower.contains(w))
}

fn is_text(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md" | "mmd" | "yaml" | "yml" | "sh" | "txt" | "toml" | "json" | "py" | "html")
    )
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name == "node_modules" || name == ".git" || name == "target" {
            continue;
        }
        if path.is_dir() {
            walk(&path, out);
        } else if is_text(&path) {
            out.push(path);
        }
    }
}

fn rel(path: &Path) -> String {
    path.strip_prefix(repo())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn plugin_manifest_does_not_enable_xpia() {
    for file in [".claude-plugin/plugin.json", "claude-plugin/hooks.json"] {
        let text = read(file);
        assert!(!text.contains("XPIA_ENABLED"), "{file} sets XPIA_ENABLED");
        assert!(
            !text.contains("tools/xpia"),
            "{file} registers an XPIA shell hook"
        );
        let json: Value = serde_json::from_str(&text).unwrap();
        let mut stack = vec![&json];
        while let Some(value) = stack.pop() {
            match value {
                Value::Object(map) => {
                    if let Some(env) = map.get("env").and_then(Value::as_object) {
                        for key in env.keys() {
                            assert!(!key.starts_with("XPIA"), "{file} env block sets {key}");
                        }
                    }
                    stack.extend(map.values());
                }
                Value::Array(items) => stack.extend(items),
                _ => {}
            }
        }
    }
    // Listing the agent is allowed: it only runs when asked for.
    assert!(read(".claude-plugin/plugin.json").contains("xpia-defense.md"));
}

const AGENT_COPIES: &[&str] = &[
    "amplifier-bundle/agents/specialized/xpia-defense.md",
    "docs/claude/agents/amplihack/specialized/xpia-defense.md",
];

fn frontmatter_description(text: &str) -> String {
    let body = text.strip_prefix("---\n").expect("agent has frontmatter");
    let end = body.find("\n---").expect("frontmatter ends");
    body[..end]
        .lines()
        .find_map(|l| l.strip_prefix("description:"))
        .expect("frontmatter has a description")
        .trim()
        .to_string()
}

#[test]
fn xpia_defense_agent_is_opt_in() {
    for file in AGENT_COPIES {
        let text = read(file);
        let description = frontmatter_description(&text);
        let lower = description.to_lowercase();
        assert!(
            lower.contains("off by default") || description.contains("XPIA_ENABLED=true"),
            "{file}: description must say XPIA is opt-in: {description}"
        );
        assert!(
            !lower.contains("transparent") && !lower.contains("automatic"),
            "{file}: description must not read as automatic: {description}"
        );
        // The whole agent is about XPIA, so every line is held to the D10
        // wording, not only lines that name XPIA.
        let hits: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| flags(l))
            .map(|(n, l)| format!("{file}:{}: {l}", n + 1))
            .collect();
        assert!(
            hits.is_empty(),
            "agent reads as automatic:\n{}",
            hits.join("\n")
        );
    }
}

/// The agent is not started by `XPIA_ENABLED`; that variable controls the
/// hook scanner. The agent also must not claim near-zero false positives,
/// since false positives are why XPIA is off by default (#1878).
#[test]
fn xpia_defense_agent_does_not_overclaim() {
    for file in AGENT_COPIES {
        let text = read(file);
        let description = frontmatter_description(&text);
        assert!(
            !description.contains("or when XPIA_ENABLED"),
            "{file}: XPIA_ENABLED does not start the agent: {description}"
        );
        assert!(
            description.contains("hook scanner"),
            "{file}: description must separate the agent from the hook scanner: {description}"
        );
        let lower = text.to_lowercase();
        for claim in ["zero false positives", "<0.1%"] {
            assert!(
                !lower.contains(claim),
                "{file}: unmeasured false-positive claim {claim:?} contradicts #1878"
            );
        }
    }
}

/// The unrecognised-value warning goes to stderr of a hook that exits 0,
/// which Claude Code and Copilot do not show. The docs must say so rather
/// than promise that a typo is noticed (#1737).
#[test]
fn xpia_docs_do_not_promise_a_visible_warning() {
    let mut files: Vec<&str> = AGENT_COPIES.to_vec();
    files.push("docs/reference/xpia-configuration.md");
    files.push("docs/claude/commands/amplihack/xpia.md");
    for file in files {
        let text = read(file);
        assert!(
            !text.to_lowercase().contains("does not go unnoticed"),
            "{file}: the stderr warning is not shown in the agent session"
        );
        assert!(
            text.contains("amplihack-hooks pre-tool-use"),
            "{file}: must tell the user to run the hook by hand to see the warning"
        );
    }
}

#[test]
fn xpia_defense_agent_copies_are_identical() {
    assert_eq!(
        fs::read(repo().join(AGENT_COPIES[0])).unwrap(),
        fs::read(repo().join(AGENT_COPIES[1])).unwrap(),
        "the two xpia-defense.md copies must stay byte-identical"
    );
}

#[test]
fn xpia_command_doc_is_opt_in() {
    let mut copies = vec![repo().join("docs/claude/commands/amplihack/xpia.md")];
    for extra in [
        "amplifier-bundle/commands/amplihack/xpia.md",
        "amplifier-bundle/commands/xpia.md",
    ] {
        let path = repo().join(extra);
        if path.exists() {
            copies.push(path);
        }
    }
    for path in copies {
        let text = fs::read_to_string(&path).unwrap();
        let hits: Vec<String> = text
            .lines()
            .enumerate()
            .filter(|(_, l)| flags(l))
            .map(|(n, l)| format!("{}:{}: {l}", rel(&path), n + 1))
            .collect();
        assert!(
            hits.is_empty(),
            "/xpia reads as on by default:\n{}",
            hits.join("\n")
        );
        assert!(
            text.contains("XPIA_ENABLED=true"),
            "{} must say how to opt in",
            rel(&path)
        );
    }
}

#[test]
fn xpia_docs_do_not_claim_default_on() {
    let root = repo();
    let mut files = Vec::new();
    walk(&root.join("docs"), &mut files);
    walk(&root.join("amplifier-bundle"), &mut files);
    files.push(root.join("README.md"));
    assert!(
        files.len() > 50,
        "the walk found too few files: {}",
        files.len()
    );

    let mut hits = Vec::new();
    for path in &files {
        let name = rel(path);
        if ALLOWLIST.iter().any(|(f, _)| *f == name) {
            continue;
        }
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        for (n, line) in text.lines().enumerate() {
            if line.to_lowercase().contains("xpia") && flags(line) {
                hits.push(format!("{name}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "{} line(s) describe XPIA as on by default:\n{}",
        hits.len(),
        hits.join("\n")
    );
}

#[test]
fn xpia_docs_grep_flags_known_bad_lines() {
    // The grep itself: the wording it must catch, and the wording it must pass.
    for bad in [
        "XPIA is enabled by default.",
        "XPIA hooks are automatically configured during UVX installation",
        "Transparent XPIA protection for every tool call",
        "XPIA is always-on",
    ] {
        assert!(flags(bad), "must flag: {bad}");
    }
    for good in [
        "XPIA is off by default; set XPIA_ENABLED=true to opt in.",
        "XPIA (opt-in) scans tool calls",
        "XPIA runs automatically once you set XPIA_ENABLED=true",
    ] {
        assert!(!flags(good), "must pass: {good}");
    }
}

#[test]
fn xpia_configuration_reference_is_off_by_default() {
    let text = read("docs/reference/xpia-configuration.md");
    let lower = text.to_lowercase();
    assert!(lower.contains("off by default"), "must say off by default");
    assert!(text.contains("XPIA_ENABLED=true"), "must say how to opt in");
    for line in text.lines() {
        let l = line.to_lowercase();
        assert!(
            !(l.contains("xpia_enabled") && l.contains("always passes")),
            "the health check no longer always passes (#1737 item 2): {line}"
        );
    }
    let updated = text
        .lines()
        .find_map(|l| l.strip_prefix("last_updated: "))
        .expect("front matter has last_updated");
    // ISO dates compare correctly as strings; later edits may bump it again.
    assert!(
        updated.trim() >= "2026-11-02",
        "last_updated must be bumped past the pre-#1878 2026-10-31: {updated}"
    );
}

#[test]
fn journey_hooks_diagram_labels_xpia_opt_in() {
    for file in [
        "docs/atlas/user-journeys/journey-hooks.mmd",
        "docs/atlas/user-journeys/journey-hooks-mermaid.svg",
    ] {
        let text = read(file);
        if text.to_lowercase().contains("xpia") {
            assert!(
                text.contains("xpia (opt-in)"),
                "{file} must label XPIA opt-in"
            );
        }
    }
}

/// D12, #1920: the legacy `install_with_xpia.sh` scripts were removed. They
/// could not complete an install: the bundle copy cloned the repository into
/// `./tmpamplihack`, backed up `settings.json` and then failed copying a
/// `.claude/` directory amplihack-rs does not have. Neither copy may return.
#[test]
fn install_with_xpia_scripts_are_removed() {
    for file in [
        "amplifier-bundle/tools/amplihack/install_with_xpia.sh",
        "docs/claude/tools/amplihack/install_with_xpia.sh",
    ] {
        assert!(
            !repo().join(file).exists(),
            "{file}: removed in #1920; use `amplihack install`"
        );
    }
    let reference = read("docs/reference/xpia-configuration.md");
    assert!(
        !reference
            .lines()
            .any(|l| l.starts_with("| `install_with_xpia.sh`")),
        "surfaces table must not list the removed install_with_xpia.sh"
    );
    assert!(
        reference.contains("`install_with_xpia.sh` scripts were removed (#1920)"),
        "reference must say the scripts were removed"
    );
}

/// The `/amplihack:xpia` command tells the agent what to run. It must not
/// send it to the Python tools removed in #637, or to `install_with_xpia.sh`,
/// which was removed in #1920 (#1889 review).
#[test]
fn xpia_command_doc_runs_only_tools_that_exist() {
    let file = "docs/claude/commands/amplihack/xpia.md";
    let text = read(file);
    for (n, line) in text.lines().enumerate() {
        for dead in [
            "python3 ~/.claude/tools/xpia/",
            "python3 ~/.claude/tests/run_xpia_tests.py",
            "~/.claude/tools/amplihack/install_with_xpia.sh",
            "tools/xpia/hooks/",
            "xpia_security.yaml",
            "Specs/xpia_defense_interface.py",
            "logs/xpia/security_",
        ] {
            assert!(
                !line.contains(dead),
                "{file}:{}: points at a removed tool or file {dead:?}: {line}",
                n + 1
            );
        }
    }
    assert!(
        text.contains("amplihack-hooks pre-tool-use"),
        "{file}: must give the hook command that works"
    );
}
