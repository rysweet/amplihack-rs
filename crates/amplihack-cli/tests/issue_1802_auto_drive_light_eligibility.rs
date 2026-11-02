//! Issue #1802: `amplifier-bundle/tools/autodrive_light_eligibility.sh`.
//!
//! The tool is a stateless measurement of `base..head`:
//!
//! ```text
//! autodrive_light_eligibility.sh --repo <dir> --base <rev> --head <rev>
//!     [--max-lines N] [--max-new-modules N] [--max-public-api N]
//!     [--max-schema N] [--max-recipe-contract N]
//! ```
//!
//! On a decision it exits 0 and prints two lines,
//! `AUTODRIVE_LIGHT_ELIGIBILITY=<light|escalate> reason=<token>` and one JSON
//! line with every measurement and the limits. A fault exits 2, prints
//! nothing on stdout, and prints `AUTODRIVE_LIGHT_ELIGIBILITY=fault
//! reason=<token>` on stderr. Each test commits a change in a temporary
//! repository from [`LightRepo`] and runs the real tool isolated.

use crate::support::auto_drive_light::{LightRepo, decision, fault, git, show, stdout};

const LIGHT: &str = "AUTODRIVE_LIGHT_ELIGIBILITY=light reason=within-limits";

fn escalate(reason: &str) -> String {
    format!("AUTODRIVE_LIGHT_ELIGIBILITY=escalate reason={reason}")
}

fn lines(n: usize) -> String {
    (0..n).map(|i| format!("line {i}\n")).collect()
}

// --- decision line, JSON shape and defaults --------------------------------

#[test]
fn small_doc_change_is_light_and_reports_measurements_and_default_limits() {
    let r = LightRepo::new();
    r.write("docs/guide.md", "# guide\n\nOne more line.\n");
    let head = r.commit("doc");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["decision"], "light");
    assert_eq!(j["reason"], "within-limits");
    assert_eq!(j["base"], r.base.as_str());
    assert_eq!(j["head"], head.as_str());
    assert_eq!(j["lines"], 2);
    for k in ["new_modules", "public_api", "schema", "recipe_contract"] {
        assert_eq!(j[k], 0, "{k}");
    }
    assert_eq!(
        j["limits"],
        serde_json::json!({"lines":300,"new_modules":0,"public_api":0,"schema":0,"recipe_contract":0})
    );
}

#[test]
fn tool_writes_nothing_to_the_repository() {
    let r = LightRepo::new();
    r.write("docs/guide.md", &lines(400));
    r.commit("big doc");
    let before = git(&r.wt, &["status", "--porcelain", "--ignored"]);
    let refs_before = git(&r.wt, &["for-each-ref"]);
    decision(&r.measure(&[]), &escalate("lines"));
    assert_eq!(git(&r.wt, &["status", "--porcelain", "--ignored"]), before);
    assert_eq!(git(&r.wt, &["for-each-ref"]), refs_before);
}

// --- lines -----------------------------------------------------------------

#[test]
fn line_limit_is_inclusive_300_passes_and_301_escalates() {
    let r = LightRepo::new();
    r.write("docs/a.md", &lines(300));
    r.commit("300");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["lines"], 300);

    r.write("docs/b.md", "one more\n");
    r.commit("301");
    let j = decision(&r.measure(&[]), &escalate("lines"));
    assert_eq!(j["decision"], "escalate");
    assert_eq!(j["lines"], 301);
}

#[test]
fn added_and_deleted_lines_are_summed() {
    let r = LightRepo::new();
    r.write("docs/guide.md", "# replaced\n");
    r.commit("replace");
    // One line deleted, one added.
    assert_eq!(decision(&r.measure(&[]), LIGHT)["lines"], 2);
}

#[test]
fn lock_files_are_excluded_from_the_line_count() {
    let r = LightRepo::new();
    r.write("Cargo.lock", &lines(1000));
    r.write("web/yarn.lock", &lines(500));
    r.write("web/package-lock.json", &lines(500));
    r.write("docs/guide.md", "# guide\nx\n");
    r.commit("relock");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["lines"], 1);
}

#[test]
fn binary_files_count_zero_lines() {
    let r = LightRepo::new();
    let blob: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
    r.write_bytes("docs/img.bin", &blob);
    r.commit("binary");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["lines"], 0);
}

#[test]
fn a_file_name_with_a_newline_is_measured_correctly() {
    let r = LightRepo::new();
    r.write("docs/odd\nname.md", "a\nb\n");
    r.commit("odd name");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["lines"], 2);
}

#[test]
fn line_limit_override_and_base_ten_reading() {
    let r = LightRepo::new();
    r.write("docs/a.md", &lines(350));
    r.commit("350");
    decision(&r.measure(&[]), &escalate("lines"));
    let j = decision(&r.measure(&["--max-lines", "400"]), LIGHT);
    assert_eq!(j["limits"]["lines"], 400);

    let r = LightRepo::new();
    r.write("docs/a.md", &lines(10));
    r.commit("10");
    // `010` is ten, not octal eight.
    let j = decision(&r.measure(&["--max-lines", "010"]), LIGHT);
    assert_eq!(j["limits"]["lines"], 10);
    r.write("docs/b.md", "x\n");
    r.commit("11");
    decision(&r.measure(&["--max-lines", "010"]), &escalate("lines"));
}

// --- new modules -----------------------------------------------------------

#[test]
fn each_kind_of_new_module_escalates() {
    for path in [
        "crates/foo/src/added.rs",
        "crates/foo/src/deep/nested/mod.rs",
        "crates/bar/Cargo.toml",
        "amplifier-bundle/recipes/new-recipe.yaml",
        "amplifier-bundle/tools/new_tool.sh",
        "amplifier-bundle/tools/sub/helper.py",
    ] {
        let r = LightRepo::new();
        r.write(path, "x\n");
        r.commit("add");
        let j = decision(&r.measure(&[]), &escalate("new-module"));
        assert_eq!(j["new_modules"], 1, "{path}");
    }
}

#[test]
fn a_new_module_with_a_newline_in_its_name_escalates() {
    let r = LightRepo::new();
    r.write("crates/foo/src/odd\nname.rs", "x\n");
    r.commit("add");
    decision(&r.measure(&[]), &escalate("new-module"));
}

#[test]
fn added_tests_and_docs_are_not_new_modules() {
    let r = LightRepo::new();
    r.write("crates/foo/tests/new_test.rs", "#[test]\nfn t() {}\n");
    r.write("tests/integration/extra.rs", "fn x() {}\n");
    r.write("docs/reference/new-page.md", "# new\n");
    r.commit("tests and docs");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["new_modules"], 0);
}

#[test]
fn new_module_limit_can_be_raised() {
    let r = LightRepo::new();
    r.write("crates/foo/src/added.rs", "x\n");
    r.commit("add");
    let j = decision(&r.measure(&["--max-new-modules", "1"]), LIGHT);
    assert_eq!(j["new_modules"], 1);
}

// --- public API ------------------------------------------------------------

#[test]
fn an_added_or_removed_pub_item_in_lib_rs_escalates() {
    for body in [
        "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
        "fn private() {}\n",
        "fn private() {}\n\npub fn kept() {}\n    pub struct Spaced;\n",
    ] {
        let r = LightRepo::new();
        r.write("crates/foo/src/lib.rs", body);
        r.commit("api");
        let j = decision(&r.measure(&[]), &escalate("public-api"));
        assert!(j["public_api"].as_u64().unwrap() >= 1, "{body:?}");
    }
}

#[test]
fn restricted_visibility_and_pub_in_other_files_do_not_count() {
    let r = LightRepo::new();
    r.write(
        "crates/foo/src/lib.rs",
        "fn private() {}\n\npub fn kept() {}\npub(crate) fn a() {}\npub(super) fn b() {}\npub(in crate::x) fn c() {}\n// pub fn in a comment\npublic_thing();\n",
    );
    r.write(
        "crates/foo/src/existing.rs",
        "fn helper() {}\npub fn elsewhere() {}\n",
    );
    r.commit("restricted");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["public_api"], 0);
}

// --- schema ----------------------------------------------------------------

#[test]
fn schema_files_escalate() {
    for path in [
        "config.schema.json",
        "crates/foo/data/round.schema.json",
        "schemas/record.json",
        "amplifier-bundle/schemas/any.yaml",
    ] {
        let r = LightRepo::new();
        r.write(path, "{}\n");
        r.commit("schema");
        let j = decision(&r.measure(&[]), &escalate("schema"));
        assert_eq!(j["schema"], 1, "{path}");
    }
}

// --- recipe contract -------------------------------------------------------

#[test]
fn a_changed_context_key_set_escalates() {
    for body in [
        // Added key.
        "name: demo\ncontext:\n  alpha: \"1\"\n  beta: \"\"\n  gamma: \"\"\nsteps:\n  - id: one\n    type: bash\n    command: echo one\n",
        // Removed key.
        "name: demo\ncontext:\n  alpha: \"1\"\nsteps:\n  - id: one\n    type: bash\n    command: echo one\n",
        // Renamed key.
        "name: demo\ncontext:\n  alpha: \"1\"\n  bravo: \"\"\nsteps:\n  - id: one\n    type: bash\n    command: echo one\n",
    ] {
        let r = LightRepo::new();
        r.write("amplifier-bundle/recipes/demo.yaml", body);
        r.commit("contract");
        let j = decision(&r.measure(&[]), &escalate("recipe-contract"));
        assert_eq!(j["recipe_contract"], 1);
    }
}

#[test]
fn edits_to_existing_recipes_and_tools_that_keep_the_key_set_are_light() {
    let r = LightRepo::new();
    // Changed default and changed step body, same key set.
    r.write(
        "amplifier-bundle/recipes/demo.yaml",
        "name: demo\ncontext:\n  alpha: \"2\"\n  beta: \"x\"\nsteps:\n  - id: one\n    type: bash\n    command: echo changed\n",
    );
    r.write(
        "amplifier-bundle/tools/existing.sh",
        "#!/bin/sh\necho changed\n",
    );
    r.commit("edit");
    let j = decision(&r.measure(&[]), LIGHT);
    assert_eq!(j["recipe_contract"], 0);
    assert_eq!(j["new_modules"], 0);
}

#[test]
fn a_changed_inline_context_map_escalates() {
    let mut r = LightRepo::new();
    let base = "name: inline\ncontext: {alpha: \"1\"}\nsteps: []\n";
    r.write("amplifier-bundle/recipes/inline.yaml", base);
    r.base = r.commit("inline base");
    r.write(
        "amplifier-bundle/recipes/inline.yaml",
        "name: inline\ncontext: {alpha: \"1\", beta: \"\"}\nsteps: []\n",
    );
    r.commit("inline contract");
    let j = decision(&r.measure(&[]), &escalate("recipe-contract"));
    assert_eq!(j["recipe_contract"], 1);
}

#[test]
fn a_failed_secondary_diff_is_a_fault_not_a_light_change() {
    // A git that fails only the added-files diff: before the fix that diff
    // read as "no new modules" and the change measured light.
    let r = LightRepo::new();
    r.write("crates/foo/src/added.rs", "fn x() {}\n");
    let head = r.commit("new module");
    let real = crate::isolated_amplihack::isolated("sh")
        .args(["-c", "command -v git"])
        .output()
        .expect("find git");
    let real = String::from_utf8(real.stdout).unwrap().trim().to_string();
    assert!(!real.is_empty(), "git must be on PATH");
    let bin = r.tmp.path().join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let fake = bin.join("git");
    std::fs::write(
        &fake,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --diff-filter=A ] && exit 128; done\nexec '{real}' \"$@\"\n"
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .unwrap();
    let o = crate::isolated_amplihack::isolated("bash")
        .arg(crate::support::auto_drive_light::eligibility_tool())
        .args([
            "--repo",
            r.wt.to_str().unwrap(),
            "--base",
            &r.base,
            "--head",
            &head,
        ])
        .env("HOME", &r.home)
        .env("PATH", path)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("run autodrive_light_eligibility.sh");
    fault(&o, "bad-rev");
}

// --- rule order ------------------------------------------------------------

#[test]
fn reason_names_the_first_rule_in_table_order_and_json_has_all() {
    let r = LightRepo::new();
    r.write("docs/a.md", &lines(400));
    r.write("crates/foo/src/added.rs", "x\n");
    r.write(
        "crates/foo/src/lib.rs",
        "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
    );
    r.write("schemas/s.json", "{}\n");
    r.commit("everything");
    let j = decision(&r.measure(&[]), &escalate("lines"));
    assert_eq!(j["new_modules"], 1);
    assert!(j["public_api"].as_u64().unwrap() >= 1);
    assert_eq!(j["schema"], 1);

    let r = LightRepo::new();
    r.write("schemas/s.json", "{}\n");
    r.write(
        "crates/foo/src/lib.rs",
        "fn private() {}\n\npub fn kept() {}\npub fn added() {}\n",
    );
    r.commit("api and schema");
    decision(&r.measure(&[]), &escalate("public-api"));
}

// --- faults ----------------------------------------------------------------

fn args(r: &LightRepo, base: &str, head: &str, extra: &[&str]) -> Vec<String> {
    let mut v = vec![
        "--repo".to_string(),
        r.wt.to_string_lossy().into_owned(),
        "--base".into(),
        base.into(),
        "--head".into(),
        head.into(),
    ];
    v.extend(extra.iter().map(|s| s.to_string()));
    v
}

#[test]
fn a_base_that_is_not_an_ancestor_is_a_fault() {
    let r = LightRepo::new();
    let first = r.commit("first");
    git(&r.wt, &["checkout", "-q", "-b", "side", &r.base]);
    r.write("docs/side.md", "x\n");
    let side = r.commit("side");
    fault(
        &r.eligibility(&args(&r, &first, &side, &[])),
        "not-ancestor",
    );
}

#[test]
fn an_unresolvable_revision_is_a_fault() {
    let r = LightRepo::new();
    let head = r.commit("c");
    fault(
        &r.eligibility(&args(
            &r,
            "0123456789abcdef0123456789abcdef01234567",
            &head,
            &[],
        )),
        "bad-rev",
    );
    fault(
        &r.eligibility(&args(&r, &r.base, "no-such-branch", &[])),
        "bad-rev",
    );
}

#[test]
fn a_directory_that_is_not_a_repository_is_a_fault() {
    let r = LightRepo::new();
    let not_repo = r.tmp.path().join("plain");
    std::fs::create_dir_all(&not_repo).unwrap();
    let o = r.eligibility(&[
        "--repo".into(),
        not_repo.to_string_lossy().into_owned(),
        "--base".into(),
        r.base.clone(),
        "--head".into(),
        r.base.clone(),
    ]);
    fault(&o, "not-a-repo");
}

#[test]
fn option_shaped_revisions_are_refused_and_never_reach_git() {
    let r = LightRepo::new();
    let head = r.commit("c");
    let marker = r.tmp.path().join("injected");
    let inj = format!("--output={}", marker.display());
    fault(&r.eligibility(&args(&r, &inj, &head, &[])), "bad-arg");
    fault(&r.eligibility(&args(&r, &r.base, &inj, &[])), "bad-arg");
    assert!(!marker.exists(), "an option-shaped revision reached git");
}

#[test]
fn malformed_limits_and_unknown_options_are_faults() {
    let r = LightRepo::new();
    let head = r.commit("c");
    for extra in [
        &["--max-lines", "abc"][..],
        &["--max-lines", "1234567"],
        &["--max-lines", ""],
        &["--max-lines", "-1"],
        &["--max-schema", "1e3"],
        &["--max-public-api", " 1"],
        &["--max-lines"],
        &["--force-light"],
    ] {
        let o = r.eligibility(&args(&r, &r.base, &head, extra));
        fault(&o, "bad-arg");
        assert!(stdout(&o).is_empty(), "{extra:?}: {}", show(&o));
    }
}

#[test]
fn missing_required_options_are_faults() {
    let r = LightRepo::new();
    let head = r.commit("c");
    let repo = r.wt.to_string_lossy().into_owned();
    fault(&r.eligibility(&[]), "bad-arg");
    fault(
        &r.eligibility(&["--repo".into(), repo.clone(), "--head".into(), head.clone()]),
        "bad-arg",
    );
    fault(
        &r.eligibility(&["--repo".into(), repo, "--base".into(), r.base.clone()]),
        "bad-arg",
    );
}
