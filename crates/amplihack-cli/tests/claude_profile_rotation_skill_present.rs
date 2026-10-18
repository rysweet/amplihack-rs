//! Issue #1592: the bundled `claude-profile-rotation` skill exists, has
//! valid frontmatter at byte 0 with a `name` matching its directory, and
//! cites only flags that `amplihack profile rotate` accepts. Read-only.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use serde_yaml::Value;

const SKILL_NAME: &str = "claude-profile-rotation";

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

fn skill_md() -> String {
    let p = workspace_root().join(format!("amplifier-bundle/skills/{SKILL_NAME}/SKILL.md"));
    fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
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

/// See `issue_1569_auto_switch.rs`: the binary lives in `bins/amplihack`.
fn amplihack_bin() -> PathBuf {
    if let Some(p) = std::env::var_os("AMPLIHACK_TEST_BINARY") {
        return PathBuf::from(p);
    }
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--quiet", "-p", "amplihack", "--bin", "amplihack"])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .status()
        .expect("cargo build");
    assert!(status.success());
    std::env::current_exe()
        .unwrap()
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("amplihack")
}

fn help(args: &[&str]) -> String {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(amplihack_bin())
        .args(args)
        .arg("--help")
        .env_clear()
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
    assert!(desc.contains("rotat"), "{desc}");
    for (k, v) in &fm {
        assert!(
            !matches!(v, Value::Number(_) | Value::Bool(_)),
            "non-string scalar field {k:?}"
        );
    }
}

#[test]
fn skill_covers_the_commands_and_the_credential_rule() {
    let content = skill_md();
    for want in [
        "amplihack profile rotate --dry-run",
        "amplihack profile rotate start",
        "amplihack profile rotate status",
        "amplihack profile rotate stop",
        "ls -li",
        "ALERT",
    ] {
        assert!(content.contains(want), "SKILL.md is missing {want:?}");
    }
    let lower = content.to_lowercase();
    assert!(lower.contains("never") && lower.contains(".credentials.json"));
}

#[test]
fn every_flag_the_skill_cites_exists() {
    let content = skill_md();
    let known = format!(
        "{}\n{}",
        help(&["profile", "rotate"]),
        help(&["profile", "rotate", "start"])
    );
    let re = regex::Regex::new(r"(?:^|[\s`(])(--[a-z][a-z-]+)").unwrap();
    let mut cited: Vec<&str> = re
        .captures_iter(&content)
        .map(|c| c.get(1).unwrap().as_str())
        .filter(|f| *f != "--help")
        .collect();
    cited.sort();
    cited.dedup();
    assert!(!cited.is_empty());
    for f in cited {
        // Flags of other amplihack commands the skill mentions.
        if matches!(f, "--json" | "--yes") {
            continue;
        }
        assert!(
            known.contains(f),
            "SKILL.md cites {f}, which rotate does not accept"
        );
    }
    assert!(
        !content.contains("--spawned"),
        "the hidden flag is not for users"
    );
}
