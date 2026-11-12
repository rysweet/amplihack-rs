//! Issue #1740: the size, interface, manifest and path rules of
//! `amplifier-bundle/tools/quick_change_diff_limits.sh`, run against a
//! temporary repository from [`crate::support::quick_change_repo`]. The
//! guards that fail closed (drift, empty diff, faults) are in
//! `issue_1740_diff_limits_guards`. The `--committed` tests cover the
//! re-measure that runs after hook repair may have added commits.

use crate::support::quick_change_repo::{Repo, handover, stderr, verdict};

#[test]
fn one_doc_file_proceeds_and_reports_the_counts() {
    let r = Repo::new();
    r.write("README.md", "# demo\n\nFixed link.\n");
    let j = verdict(
        &r.measure(&[]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
    assert_eq!(j["verdict"], "proceed");
    assert_eq!(j["files"], 1);
    assert_eq!(j["lines"], 2);
    assert_eq!(j["max_files"], 3);
    assert_eq!(j["max_lines"], 150);
    assert_eq!(j["base"], r.base.as_str());
}

#[test]
fn default_base_is_the_merge_base_with_origin() {
    let r = Repo::new();
    r.write("README.md", "# demo!\n");
    let o = r.run(&[r.wt.to_str().unwrap()], true);
    let j = verdict(&o, 0, "QUICK_CHANGE_VERDICT=proceed reason=within-limits");
    assert_eq!(j["base"], r.base.as_str());
}

#[test]
fn large_change_to_an_existing_lockfile_proceeds() {
    let r = Repo::new();
    r.write("Cargo.lock", &"# relocked\n".repeat(600));
    let j = verdict(
        &r.measure(&[]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
    assert_eq!(j["lines"], 0);
    assert!(j["lockfile_lines"].as_u64().unwrap() >= 600);
}

#[test]
fn a_new_lockfile_gets_no_exemption() {
    let r = Repo::new();
    r.write("web/package-lock.json", &"{}\n".repeat(200));
    handover(&r, "lines>150");
}

#[test]
fn private_to_pub_crate_proceeds() {
    let r = Repo::new();
    r.write(
        "src/lib.rs",
        "pub(crate) fn private() {}\n\npub fn kept() {}\n",
    );
    verdict(
        &r.measure(&[]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
}

#[test]
fn file_limit_hands_over_and_wins_over_later_rules() {
    let r = Repo::new();
    for i in 0..3 {
        r.write(&format!("docs/{i}.md"), "x\n");
    }
    r.write(
        "src/lib.rs",
        "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
    );
    handover(&r, "files>3");
}

#[test]
fn line_limit_hands_over_and_tunes() {
    let r = Repo::new();
    r.write("docs/long.md", &"line\n".repeat(200));
    handover(&r, "lines>150");
    verdict(
        &r.measure(&["--max-lines", "300"]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
}

#[test]
fn untracked_files_are_counted_and_ignored_files_are_not() {
    let r = Repo::new();
    r.write("NOTES.md", "a\nb\n");
    r.write("target/huge.txt", &"x\n".repeat(1000));
    let j = verdict(
        &r.measure(&[]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
    assert_eq!(j["files"], 1);
    assert_eq!(j["lines"], 2);
}

#[test]
fn public_interface_added_removed_or_changed_hands_over() {
    for body in [
        "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
        "fn private() {}\n",
        "fn private() {}\n\npub fn kept(x: u8) {}\n",
        "fn private() {}\n\npub fn kept() {}\n    pub   async fn spaced() {}\n",
    ] {
        let r = Repo::new();
        r.write("src/lib.rs", body);
        handover(&r, "public-interface");
    }
}

#[test]
fn cli_attribute_change_hands_over() {
    let r = Repo::new();
    r.write(
        "src/lib.rs",
        "fn private() {}\n\npub fn kept() {}\n#[arg(long)]\n",
    );
    handover(&r, "cli-definition");
}

#[test]
fn manifest_change_hands_over() {
    let r = Repo::new();
    r.write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.1\"\n",
    );
    handover(&r, "manifest");
}

#[test]
fn sensitive_paths_hand_over() {
    for path in [
        ".github/workflows/ci.yml",
        "build.rs",
        ".cargo/config.toml",
        ".gitattributes",
    ] {
        let r = Repo::new();
        r.write(path, "x\n");
        handover(&r, "sensitive-path");
    }
}

#[test]
fn paths_with_tabs_and_newlines_parse_as_one_file() {
    let r = Repo::new();
    r.write("docs/a\tb\nc.md", "x\n");
    let j = verdict(
        &r.measure(&[]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
    assert_eq!(j["files"], 1);
}

#[test]
fn committed_change_within_limits_proceeds_after_several_commits() {
    let r = Repo::new();
    r.write("README.md", "# demo\n\nFixed link.\n");
    r.commit_all("quick change");
    r.write("docs/fix.md", "hook repair fix\n");
    r.commit_all("fix: address hook rejection");
    let j = verdict(
        &r.measure(&["--committed"]),
        0,
        "QUICK_CHANGE_VERDICT=proceed reason=within-limits",
    );
    assert_eq!(j["files"], 2);
}

#[test]
fn committed_change_over_the_line_limit_exits_5() {
    let r = Repo::new();
    r.write("README.md", "# demo, fixed\n");
    r.commit_all("quick change");
    r.write("docs/long.md", &"line\n".repeat(200));
    r.commit_all("fix: address hook rejection");
    let o = r.measure(&["--committed"]);
    let j = verdict(&o, 5, "QUICK_CHANGE_VERDICT=handover reason=lines>150");
    assert_eq!(j["verdict"], "handover");
    assert!(
        stderr(&o).contains("hook repair grew the change beyond the quick-change limits"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn committed_mode_needs_a_clean_worktree() {
    for (path, body) in [("README.md", "# dirty\n"), ("NEW.md", "untracked\n")] {
        let r = Repo::new();
        r.write("docs/fix.md", "x\n");
        r.commit_all("quick change");
        r.write(path, body);
        let o = r.measure(&["--committed"]);
        assert_eq!(o.status.code(), Some(2), "{path}: {}", stderr(&o));
        assert!(stderr(&o).contains("worktree not clean"), "{}", stderr(&o));
    }
}
