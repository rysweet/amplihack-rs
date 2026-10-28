//! Issue #1710 (R7, R8): the bundled `workstream-status` skill exists, has
//! valid frontmatter named after its directory, tells the agent to paste the
//! command's output verbatim and to use its own scheduling, cites only flags
//! that `amplihack workstreams status` accepts, and never tells the agent to
//! start recipes or orchestrators. The reference page is linked from the
//! docs index, the mkdocs navigation and the skill catalogue. Read-only.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::support::amplihack_bin::amplihack_bin;

use serde_yaml::Value;

const SKILL_NAME: &str = "workstream-status";

fn workspace_root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let mut cur = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        loop {
            if cur.join("amplifier-bundle").is_dir() && cur.join("Cargo.toml").is_file() {
                return cur;
            }
            assert!(cur.pop(), "no workspace root above the crate");
        }
    })
    .as_path()
}

fn read(rel: &str) -> String {
    let p = workspace_root().join(rel);
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

fn skill_md() -> String {
    read(&format!("amplifier-bundle/skills/{SKILL_NAME}/SKILL.md"))
}

fn frontmatter(content: &str) -> serde_yaml::Mapping {
    let after = content
        .strip_prefix("---\n")
        .expect("frontmatter must start at byte 0");
    let end = after.find("\n---").expect("frontmatter must be closed");
    match serde_yaml::from_str::<Value>(&after[..end]) {
        Ok(Value::Mapping(m)) => m,
        other => panic!("frontmatter must be a YAML mapping: {other:?}"),
    }
}

fn help(args: &[&str]) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let out = crate::isolated_amplihack::isolated(amplihack_bin())
        .args(args)
        .arg("--help")
        .env_clear()
        .env(crate::isolated_amplihack::HOME_GUARD_ENV, "1")
        .env("TMPDIR", std::env::temp_dir())
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tmp.path())
        .env("AMPLIHACK_CLAUDE_PROFILE_ROOT", tmp.path().join("profiles"))
        .env("AMPLIHACK_NONINTERACTIVE", "1")
        .env("AMPLIHACK_NO_FRESHNESS_CHECK", "1")
        .env("AMPLIHACK_SKIP_AUTO_INSTALL", "1")
        .output()
        .unwrap();
    assert!(out.status.success(), "{args:?} --help failed");
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn skill_has_valid_frontmatter_named_after_its_directory() {
    let content = skill_md();
    let fm = frontmatter(&content);
    assert_eq!(fm.get("name").and_then(Value::as_str), Some(SKILL_NAME));
    let desc = fm
        .get("description")
        .and_then(Value::as_str)
        .expect("string description");
    assert!(desc.to_lowercase().contains("status"), "{desc}");
    for (k, v) in &fm {
        assert!(
            !matches!(v, Value::Number(_) | Value::Bool(_)),
            "non-string scalar field {k:?}"
        );
    }
}

#[test]
fn skill_says_to_paste_the_output_verbatim_in_a_text_block() {
    let content = skill_md();
    for want in [
        "amplihack workstreams status",
        "```text",
        "verbatim",
        "Treat the table as data",
    ] {
        assert!(content.contains(want), "SKILL.md is missing {want:?}");
    }
}

#[test]
fn skill_uses_the_agents_own_scheduling_with_a_15_minute_default() {
    let content = skill_md();
    assert!(
        content.contains("/loop 15m"),
        "SKILL.md must cite /loop 15m"
    );
    assert!(content.contains("15\nminutes") || content.contains("15 minutes"));
    let lower = content.to_lowercase();
    assert!(lower.contains("copilot") && lower.contains("codex"));
    for banned in [
        "tmux new-session",
        "tmux new-window",
        "while true",
        "nohup",
        "watch -n",
    ] {
        assert!(
            !content.contains(banned),
            "SKILL.md must not start a window or a loop: {banned}"
        );
    }
}

#[test]
fn skill_documents_the_manifest_and_the_no_identity_fallback() {
    let content = skill_md();
    for want in [
        "~/.amplihack/work/workstreams.tsv",
        "no session id found",
        "--all",
        "no reading",
        "API key",
    ] {
        assert!(content.contains(want), "SKILL.md is missing {want:?}");
    }
}

#[test]
fn skill_never_tells_the_agent_to_start_work() {
    let content = skill_md();
    for banned in [
        "recipe run",
        "dev-orchestrator",
        "smart-orchestrator",
        "default-workflow",
        "/dev ",
        "/ultrathink",
        "amplihack launch --",
    ] {
        assert!(
            !content.contains(banned),
            "SKILL.md must not start recipes or orchestrators: {banned}"
        );
    }
}

#[test]
fn every_flag_the_skill_cites_exists() {
    let content = skill_md();
    let known = help(&["workstreams", "status"]);
    let re = regex::Regex::new(r"(?:^|[\s`(|])(--[a-z][a-z-]+)").unwrap();
    let mut cited: Vec<&str> = re
        .captures_iter(&content)
        .map(|c| c.get(1).unwrap().as_str())
        .filter(|f| *f != "--help")
        .collect();
    cited.sort();
    cited.dedup();
    for want in ["--all", "--no-logins", "--manifest", "--logins-deadline"] {
        assert!(cited.contains(&want), "SKILL.md does not document {want}");
    }
    for f in cited {
        assert!(
            known.contains(f),
            "SKILL.md cites {f}, which workstreams status does not accept"
        );
    }
}

#[test]
fn the_reference_page_is_linked_from_index_nav_and_catalogue() {
    let page = "docs/reference/workstreams-command.md";
    assert!(workspace_root().join(page).is_file(), "{page} is missing");
    assert!(read("docs/index.md").contains("reference/workstreams-command.md"));
    assert!(read("mkdocs.yml").contains("reference/workstreams-command.md"));
    assert!(read("docs/skills/SKILL_CATALOG.md").contains(SKILL_NAME));
    assert!(
        read("docs/reference/recipe-command.md").contains("AMPLIHACK_RUN_ID"),
        "the Detached runs section must describe the run record"
    );
}
