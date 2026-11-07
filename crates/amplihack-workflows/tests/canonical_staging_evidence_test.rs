// File: crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs
//! Consistency checks for separately produced acceptance evidence. These checks
//! do not execute delivery workflows or turn mock receipts into live acceptance.
#![cfg(unix)]
#[path = "support/canonical_staging.rs"]
mod support;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use support::*;

// Exact existing task assets and the owner-authorised integration maintenance list.
const EXISTING_TASK_TEST_PATHS: &[&str] = &[
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/maintenance_paths.json",
    "amplifier-bundle/recipes/tests/canonical-staging.md",
    "amplifier-bundle/recipes/tests/canonical_staging_checkpoint_audit.py",
    "amplifier-bundle/recipes/tests/canonical_staging_conservative_delta.md",
    "amplifier-bundle/recipes/tests/canonical_staging_publication.py",
    "amplifier-bundle/recipes/tests/canonical_staging_synthesis.md",
    "amplifier-bundle/recipes/tests/checkpoint-pre-delta-assets.json",
    "amplifier-bundle/recipes/tests/minimalist-auto-stage.md",
    "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-conservative-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-publication-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-rebase-identity.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-refusal-evidence.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-review-boundaries.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-scope-history.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-scope.py",
    "amplifier-bundle/recipes/tests/test-checkpoint-scoped-format.sh",
    "amplifier-bundle/recipes/tests/test-checkpoint-synthesis-alias.py",
    "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-crusty-directories.py",
    "amplifier-bundle/recipes/tests/test-pragmatic-evidence.py",
    "crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs",
    "crates/amplihack-workflows/tests/canonical_staging_evidence_test.rs",
    "crates/amplihack-workflows/tests/canonical_staging_policy_test.rs",
    "crates/amplihack-workflows/tests/canonical_staging_runner_test.rs",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/README.md",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/audit_receipts.py",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/baseline_asset_sha256.json",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/coverage.json",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/inventory.json",
    "crates/amplihack-workflows/tests/fixtures/canonical_staging/private_adapter.py",
    "crates/amplihack-workflows/tests/support/canonical_staging.rs",
];

const CHECKPOINT_DELTA: &[(&str, &str, &str, &str)] = &[
    (
        "workflow-publish",
        "step-15-commit-push",
        "workflow_publish_checkpoint.sh",
        "publish",
    ),
    (
        "workflow-pr-review",
        "step-18c-push-feedback-changes",
        "workflow_review_checkpoint.sh",
        "review",
    ),
    (
        "workflow-finalize",
        "step-20b-push-cleanup",
        "workflow_cleanup_checkpoint.sh",
        "cleanup",
    ),
];

fn literal_step_block<'a>(text: &'a str, id: &str) -> &'a str {
    let marker = format!("  - id: \"{id}\"\n");
    assert_eq!(
        text.matches(&marker).count(),
        1,
        "missing or repeated step: {id}"
    );
    let start = text.find(&marker).unwrap();
    let rest = &text[start..];
    let end = rest.find("\n  - id: ").map_or(rest.len(), |i| i + 1);
    &rest[..end]
}

fn literal_command(block: &str) -> &str {
    let start = block.find("    command: |\n").unwrap();
    let body = start + "    command: |\n".len();
    let mut end = body;
    for line in block[body..].split_inclusive('\n') {
        if !line.trim().is_empty() && !line.starts_with("      ") {
            break;
        }
        end += line.len();
    }
    &block[start..end]
}

fn restore_auxiliary_extraction(asset: &str, text: String) -> String {
    let (id, helper, wrapper) = match asset {
        "workflow-prep" => (
            "step-00-workflow-preparation",
            "workflow_prepare_banner.sh",
            r#"PREP_TOOL=""
for c in "${AMPLIHACK_HOME:-}" "${REPO_PATH:-}" "$(pwd)" "${HOME:-/root}/.copilot" "${HOME:-/root}/.amplihack"; do
  if [ -n "$c" ] && [ -f "$c/amplifier-bundle/tools/workflow_prepare_banner.sh" ]; then PREP_TOOL="$c/amplifier-bundle/tools/workflow_prepare_banner.sh"; break; fi
done
[ -n "$PREP_TOOL" ] || { echo 'ERROR: workflow preparation helper not found' >&2; exit 2; }
bash "$PREP_TOOL"
"#,
        ),
        "autodrive-crusty-round" => (
            "step-01-round-context",
            "autodrive_round_context.sh",
            "bash \"/${AUTODRIVE_LAUNCH_TOOLS#/}/autodrive_round_context.sh\"\n",
        ),
        _ => return text,
    };
    let original = scope_audit_git(&[
        "show",
        &format!("606f3c746e51b8ef66d75380b41791b9f3a9ed8e:amplifier-bundle/recipes/{asset}.yaml"),
    ]);
    let original = String::from_utf8(original).unwrap();
    let before: serde_yaml::Value = serde_yaml::from_str(&original).unwrap();
    let current: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let step = |doc: &serde_yaml::Value| {
        doc["steps"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|step| step["id"].as_str() == Some(id))
            .unwrap()["command"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(step(&current), wrapper, "exact auxiliary wrapper: {asset}");
    let extracted = root().join("amplifier-bundle/tools").join(helper);
    assert_eq!(
        fs::read(&extracted).unwrap(),
        format!("#!/usr/bin/env bash\n{}", step(&before)).into_bytes(),
        "the complete original command, comments and outputs must remain unchanged"
    );
    let block = literal_step_block(&text, id);
    let restored = block.replacen(
        literal_command(block),
        literal_command(literal_step_block(&original, id)),
        1,
    );
    text.replacen(block, &restored, 1)
}

/// Restore only the authorised extraction and two new bounded phase steps.
/// The existing byte-hash comparison still covers every other original byte.
fn restore_checkpoint_delta(asset: &str, text: String) -> String {
    let Some(&(_, id, helper, phase)) = CHECKPOINT_DELTA.iter().find(|entry| entry.0 == asset)
    else {
        return text;
    };
    let text = if asset == "workflow-finalize" {
        let current = r###"      # Owner-approved #1554 amendment: every executed fetch failure is terminal.
      sync_output_file=$(mktemp "${TMPDIR:?assigned TMPDIR required}/early-pr-sync.XXXXXX")
      sync_rc=0
      bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh" --strict-fetch --probe-origin-ref >"$sync_output_file" 2>&1 || sync_rc=$?
      redaction_rc=0
      sed -E 's|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g' "$sync_output_file" >&2 || redaction_rc=$?
      rm -f -- "$sync_output_file" || :
      [ "$sync_rc" -eq 0 ] || exit "$sync_rc"
      [ "$redaction_rc" -eq 0 ] || exit "$redaction_rc"
"###;
        let prior = r###"      bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh"
"###;
        assert_eq!(text.matches(current).count(), 1);
        let text = text.replacen(current, prior, 1);
        let amended =
            "  # probe accepted as ahead and set a missing upstream. Executed failures stop.\n";
        let original =
            "  # probe accepted as ahead and set a missing upstream. Never fails the run.\n";
        assert_eq!(text.matches(amended).count(), 1);
        text.replacen(amended, original, 1)
    } else {
        text
    };
    let document: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    let steps = document["steps"].as_sequence().unwrap();
    let review_id = format!("{id}-scope-review");
    let capture_id = format!("{id}-scope-capture");
    let review = steps
        .iter()
        .find(|step| step["id"].as_str() == Some(review_id.as_str()))
        .unwrap();
    assert_eq!(review["auto_stage"].as_bool(), Some(false));
    assert_eq!(
        review["working_dir"].as_str(),
        Some("{{worktree_setup.worktree_path}}")
    );
    assert_eq!(review["parse_json"].as_bool(), Some(false));
    assert_eq!(review["output"].as_str(), Some("checkpoint_scope_review"));
    let capture = steps
        .iter()
        .find(|step| step["id"].as_str() == Some(capture_id.as_str()))
        .unwrap();
    assert_eq!(capture["type"].as_str(), Some("bash"));
    assert!(capture.get("agent").is_none());
    assert_eq!(capture["parse_json"].as_bool(), Some(true));
    assert_eq!(capture["output"].as_str(), Some("checkpoint_scope_capture"));
    assert!(
        capture["command"]
            .as_str()
            .unwrap()
            .contains(&format!("--scope-capture {phase}"))
    );
    let checkpoint = steps
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .unwrap();
    assert!(checkpoint["command"].as_str().unwrap().contains(helper));
    let capture_position = steps
        .iter()
        .position(|step| step["id"].as_str() == Some(capture_id.as_str()))
        .unwrap();
    let review_position = steps
        .iter()
        .position(|step| step["id"].as_str() == Some(review_id.as_str()))
        .unwrap();
    let checkpoint_position = steps
        .iter()
        .position(|step| step["id"].as_str() == Some(id))
        .unwrap();
    assert!(capture_position < review_position && review_position < checkpoint_position);
    let original = scope_audit_git(&[
        "show",
        &format!("606f3c746e51b8ef66d75380b41791b9f3a9ed8e:amplifier-bundle/recipes/{asset}.yaml"),
    ]);
    let original = String::from_utf8(original).unwrap();
    let original_block = literal_step_block(&original, id);
    let original_document: serde_yaml::Value = serde_yaml::from_str(&original).unwrap();
    let old_step = original_document["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|step| step["id"].as_str() == Some(id))
        .unwrap();
    let extracted = fs::read_to_string(root().join("amplifier-bundle/tools").join(helper)).unwrap();
    let extracted = if asset == "workflow-publish" {
        // Decision 17.116 explicitly replaces the cached-fallback success.
        let current_failure = r#"    echo "ERROR: git fetch origin failed before ancestry check (exit $fetch_rc); refusing publication" >&2
    redact_sensitive_file "$fetch_output_file" >&2
    rm -f "$fetch_output_file"
    reason="fetch-failed"
    exit "$fetch_rc""#;
        let original_failure = r#"    echo "WARNING: git fetch origin failed before ancestry check; using cached upstream state" >&2
    redact_sensitive_file "$fetch_output_file" >&2"#;
        assert_eq!(extracted.matches(current_failure).count(), 1);
        let extracted = extracted.replacen(current_failure, original_failure, 1);
        let decision = r#"DESTINATION_IS_HEAD=$(workflow_checkpoint_python destination-is-head) || { reason="publication-destination-inspection-failed"; exit 1; }"#;
        assert_eq!(extracted.matches(decision).count(), 2);
        let extracted = extracted.replace(&format!("{decision}\n"), "");
        let extracted = extracted.replace(
            r#"if [ "$COMMITS_AHEAD" -ne 0 ] || [ "$DESTINATION_IS_HEAD" != true ]; then"#,
            r#"if [ "$COMMITS_AHEAD" -ne 0 ]; then"#,
        );
        let extracted = extracted.replace(
            r#"if [ "$COMMITS_AHEAD" -eq 0 ] && [ "$DESTINATION_IS_HEAD" = true ]; then"#,
            r#"if [ "$COMMITS_AHEAD" -eq 0 ]; then"#,
        );
        let corrected = r#"workflow_checkpoint_python verify-pushed-ref "$V" "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha""#;
        let original =
            r#"bash "$V" origin "${REMOTE_BRANCH:-$(git rev-parse --abbrev-ref HEAD)}" "$raw_sha""#;
        assert_eq!(extracted.matches(corrected).count(), 1);
        extracted.replacen(corrected, original, 1)
    } else if asset == "workflow-pr-review" {
        let first_failure = concat!(
            "    # Decision 17.116 permits this first-failure stop. Push retries are unchanged.\n",
            "    if [ \"$#\" -eq 3 ] && [ \"$1\" = git ] && [ \"$2\" = pull ] && [ \"$3\" = --rebase ]; then return \"$rc\"; fi\n"
        );
        assert_eq!(extracted.matches(first_failure).count(), 1);
        let extracted = extracted.replacen(first_failure, "", 1);
        let corrected = r#"redact_sensitive_file "$push_output_file""#;
        assert_eq!(extracted.matches(corrected).count(), 2);
        extracted.replace(corrected, r#"cat "$push_output_file""#)
    } else if asset == "workflow-finalize" {
        let strict_sync = r###"workflow_checkpoint_sync_body() {
bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh" --strict-fetch
}
sync_output_file=$(mktemp -t step20b-cleanup-sync-XXXXXX)
if workflow_checkpoint_operation sync workflow_checkpoint_sync_body >"$sync_output_file" 2>&1; then
  redact_sensitive_output < "$sync_output_file"
  rm -f -- "$sync_output_file"
else
  sync_rc=$?
  redact_sensitive_output < "$sync_output_file" >&2
  rm -f -- "$sync_output_file"
  exit "$sync_rc"
fi"###;
        let prior_sync = r###"workflow_checkpoint_sync_body() {
bash "$(dirname "$WT_TOOL")/workflow_pr_head_sync.sh"
}
workflow_checkpoint_operation sync workflow_checkpoint_sync_body || exit $?"###;
        assert_eq!(extracted.matches(strict_sync).count(), 1);
        let extracted = extracted.replacen(strict_sync, prior_sync, 1);
        assert_eq!(extracted.matches("|| workflow_cleanup_skip ").count(), 3);
        extracted.replace("|| workflow_cleanup_skip ", "|| skip ")
    } else {
        extracted
    };
    // Restore only the four authorised commit-output calls. Keep exact channels,
    // individual counts and success-before-rejection order in this audit.
    let extracted = if matches!(asset, "workflow-pr-review" | "workflow-finalize") {
        let redactor = if asset == "workflow-pr-review" {
            r#"redact_sensitive_file "$commit_output_file""#
        } else {
            r#"redact_sensitive_output < "$commit_output_file""#
        };
        assert_eq!(extracted.matches(redactor).count(), 2);
        assert!(!extracted.contains(r#"cat "$commit_output_file""#));
        let mut lines: Vec<String> = extracted.split_inclusive('\n').map(str::to_owned).collect();
        let mut positions = Vec::new();
        for suffix in ["", " >&2"] {
            let corrected = format!("    {redactor}{suffix}\n");
            let original = format!("    cat \"$commit_output_file\"{suffix}\n");
            assert_eq!(lines.iter().filter(|line| **line == corrected).count(), 1);
            let position = lines.iter().position(|line| *line == corrected).unwrap();
            positions.push(position);
            lines[position] = original;
        }
        assert!(positions[0] < positions[1]);
        lines.concat()
    } else {
        extracted
    };
    let corrected_redaction = r"s|([A-Za-z][A-Za-z0-9+.-]*://)[^/[:space:]?#]*@|\1REDACTED@|g";
    let original_redaction = r"s#(https?://)[^@[:space:]]+@#\1REDACTED@#g";
    assert_eq!(extracted.matches(corrected_redaction).count(), 1);
    let extracted = extracted.replacen(corrected_redaction, original_redaction, 1);
    let extracted_lines: Vec<_> = extracted.lines().map(str::trim).collect();
    let mut cursor = 0;
    for line in old_step["command"].as_str().unwrap().lines().map(str::trim) {
        if line.is_empty()
            || line.starts_with("stage_workflow_changes")
            || line.starts_with("git add ")
        {
            continue;
        }
        let offset = extracted_lines[cursor..]
            .iter()
            .position(|candidate| *candidate == line)
            .unwrap_or_else(|| panic!("extraction removed or changed an original line: {line}"));
        cursor += offset + 1;
    }
    let mut restored = text.clone();
    for extra in [&capture_id, &review_id] {
        restored = restored.replacen(literal_step_block(&text, extra), "", 1);
    }
    let current_block = literal_step_block(&restored, id).to_owned();
    let restored_block = current_block.replacen(
        literal_command(&current_block),
        literal_command(original_block),
        1,
    );
    restored.replacen(&current_block, &restored_block, 1)
}

// This task-specific audit inspects each Git input separately. A worktree
// restored to approved bytes cannot conceal a different index or commit blob.
fn scope_audit_git_at(repository: &Path, args: &[&str]) -> Vec<u8> {
    let tmp = tempfile::tempdir().unwrap();
    let mut command = amplihack_git::command();
    isolated(&mut command, tmp.path());
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    checked(command.arg("-C").arg(repository).args(args)).stdout
}

fn scope_audit_git(args: &[&str]) -> Vec<u8> {
    scope_audit_git_at(&root(), args)
}

fn complete_scope_history(repository: &Path, base: &str) -> Vec<u8> {
    assert_eq!(
        scope_audit_git_at(repository, &["rev-parse", "--is-shallow-repository"]),
        b"false\n",
        "scope audit requires complete task history"
    );
    let grafts = scope_audit_git_at(repository, &["rev-parse", "--git-path", "info/grafts"]);
    let grafts = std::ffi::OsStr::from_bytes(grafts.strip_suffix(b"\n").unwrap_or(&grafts));
    match fs::symlink_metadata(repository.join(grafts)) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => panic!("scope audit refuses repository grafts or unreadable graft state"),
    }
    scope_audit_git_at(repository, &["merge-base", "--is-ancestor", base, "HEAD"]);
    scope_audit_git_at(repository, &["rev-list", &format!("{base}..HEAD")])
}

#[test]
#[ignore = "requires task-private evidence and temporary directories; no external tools beyond Git"]
fn scope_history_retains_replaced_transient_commits_and_refuses_grafts() {
    let f = Fixture::checkpoint_clean("scope-history-replacement");
    let base = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD"])).unwrap();
    let base = base.trim();
    fs::write(f.task.join("forbidden.txt"), "transient source\n").unwrap();
    f.git(&f.task, &["add", "--", "forbidden.txt"]);
    f.git(&f.task, &["commit", "-m", "transient forbidden source"]);
    let transient = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD"])).unwrap();
    f.git(&f.task, &["rm", "--", "forbidden.txt"]);
    f.git(&f.task, &["commit", "-m", "remove transient source"]);
    let head = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD"])).unwrap();
    let expected = complete_scope_history(&f.task, base);
    assert!(String::from_utf8_lossy(&expected).contains(transient.trim()));
    f.git(&f.task, &["replace", head.trim(), base]);
    assert_eq!(complete_scope_history(&f.task, base), expected);
    f.git(&f.task, &["replace", "-d", head.trim()]);
    let tree = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD^{tree}"])).unwrap();
    let unrelated = String::from_utf8(f.git(
        &f.task,
        &["commit-tree", tree.trim(), "-m", "unrelated retained base"],
    ))
    .unwrap();
    assert!(
        std::panic::catch_unwind(|| complete_scope_history(&f.task, unrelated.trim())).is_err()
    );
    let grafts = f.review.join(".git/info/grafts");
    fs::create_dir_all(grafts.parent().unwrap()).unwrap();
    fs::write(&grafts, format!("{} {}\n", head.trim(), base)).unwrap();
    assert!(std::panic::catch_unwind(|| complete_scope_history(&f.task, base)).is_err());
}

#[test]
#[ignore = "requires task-private evidence and temporary directories; no external tools beyond Git"]
fn scope_history_refuses_a_shallow_tip_even_when_the_base_object_is_present() {
    let f = Fixture::checkpoint_clean("scope-history-shallow");
    let base = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD"])).unwrap();
    fs::write(f.task.join("source.txt"), "later source\n").unwrap();
    f.git(&f.task, &["add", "--", "source.txt"]);
    f.git(&f.task, &["commit", "-m", "later source"]);
    let shallow = f.temp.path().join("shallow");
    f.git(
        f.temp.path(),
        &[
            "clone",
            "--depth",
            "1",
            "--branch",
            "task",
            &format!("file://{}", f.review.display()),
            shallow.to_str().unwrap(),
        ],
    );
    f.git(
        &shallow,
        &[
            "fetch",
            "--no-tags",
            &format!("file://{}", f.review.display()),
            base.trim(),
        ],
    );
    assert_eq!(
        scope_audit_git_at(&shallow, &["cat-file", "-t", base.trim()]),
        b"commit\n"
    );
    assert!(std::panic::catch_unwind(|| complete_scope_history(&shallow, base.trim())).is_err());
}

/// The parent authorised exactly one existing workspace dev-dependency edge.
/// Compare complete bytes so this does not permit other manifest or lock edits.
fn assert_approved_test_dependency_change(path: &str) {
    assert_approved_test_dependency_change_at(
        &root(),
        "606f3c746e51b8ef66d75380b41791b9f3a9ed8e",
        path,
    );
}

fn assert_approved_test_dependency_change_at(repository: &Path, base: &str, path: &str) {
    let original = scope_audit_git_at(repository, &["show", &format!("{base}:{path}")]);
    let original = String::from_utf8(original).unwrap();
    let (before, after) = match path {
        "crates/amplihack-workflows/Cargo.toml" => (
            "[dev-dependencies]\ntempfile",
            "[dev-dependencies]\namplihack-git = { workspace = true }\ntempfile",
        ),
        "Cargo.lock" => (
            "name = \"amplihack-workflows\"\nversion = \"1.2.0\"\ndependencies = [\n \"amplihack-utils\",",
            "name = \"amplihack-workflows\"\nversion = \"1.2.0\"\ndependencies = [\n \"amplihack-git\",\n \"amplihack-utils\",",
        ),
        _ => panic!("unapproved dependency path: {path}"),
    };
    assert_eq!(original.matches(before).count(), 1);
    let approved = original.replacen(before, after, 1);
    assert_eq!(
        fs::read(repository.join(path)).unwrap(),
        approved.as_bytes()
    );
    let mut revisions = vec![String::new()];
    let history = scope_audit_git_at(repository, &["rev-list", &format!("{base}..HEAD")]);
    revisions.extend(
        String::from_utf8(history)
            .unwrap()
            .lines()
            .map(str::to_owned),
    );
    for revision in revisions {
        let blob = scope_audit_git_at(repository, &["show", &format!("{revision}:{path}")]);
        assert!(
            blob == original.as_bytes() || blob == approved.as_bytes(),
            "unapproved staged or committed dependency blob: {revision}:{path}"
        );
    }
}

#[test]
#[ignore = "requires task-private evidence and temporary directories; no external tools beyond Git"]
fn dependency_baseline_ignores_replacements_and_refuses_forged_bytes() {
    let f = Fixture::checkpoint_clean("dependency-baseline-replacement");
    let manifest_path = "crates/amplihack-workflows/Cargo.toml";
    let real_base = "606f3c746e51b8ef66d75380b41791b9f3a9ed8e";
    let original_manifest = String::from_utf8(scope_audit_git(&[
        "show",
        &format!("{real_base}:{manifest_path}"),
    ]))
    .unwrap();
    let original_lock = String::from_utf8(scope_audit_git(&[
        "show",
        &format!("{real_base}:Cargo.lock"),
    ]))
    .unwrap();
    fs::create_dir_all(f.task.join("crates/amplihack-workflows")).unwrap();
    fs::write(f.task.join(manifest_path), &original_manifest).unwrap();
    fs::write(f.task.join("Cargo.lock"), &original_lock).unwrap();
    f.git(&f.task, &["add", "--", manifest_path, "Cargo.lock"]);
    f.git(
        &f.task,
        &["commit", "-m", "independent dependency fixture baseline"],
    );
    let base = String::from_utf8(f.git(&f.task, &["rev-parse", "HEAD"])).unwrap();
    let base = base.trim();
    // These exact changes are declared independently from the comparator.
    // They are the sole two-file owner permission for this task.
    let manifest_before = "[dev-dependencies]\ntempfile";
    let manifest_after = "[dev-dependencies]\namplihack-git = { workspace = true }\ntempfile";
    let lock_before = "name = \"amplihack-workflows\"\nversion = \"1.2.0\"\ndependencies = [\n \"amplihack-utils\",";
    let lock_after = "name = \"amplihack-workflows\"\nversion = \"1.2.0\"\ndependencies = [\n \"amplihack-git\",\n \"amplihack-utils\",";
    assert_eq!(original_manifest.matches(manifest_before).count(), 1);
    assert_eq!(original_lock.matches(lock_before).count(), 1);
    let legitimate_manifest = original_manifest.replacen(manifest_before, manifest_after, 1);
    let legitimate_lock = original_lock.replacen(lock_before, lock_after, 1);
    fs::write(f.task.join(manifest_path), &legitimate_manifest).unwrap();
    fs::write(f.task.join("Cargo.lock"), &legitimate_lock).unwrap();
    f.git(&f.task, &["add", "--", manifest_path, "Cargo.lock"]);
    assert_approved_test_dependency_change_at(&f.task, base, manifest_path);
    assert_approved_test_dependency_change_at(&f.task, base, "Cargo.lock");
    let legitimate_tree = String::from_utf8(f.git(&f.task, &["write-tree"])).unwrap();
    let forged = original_manifest.replacen(
        "[dev-dependencies]",
        "# Unapproved baseline content, private fixture only\n[dev-dependencies]",
        1,
    );
    let forged_file = f.temp.path().join("forged-manifest.toml");
    fs::write(&forged_file, &forged).unwrap();
    let blob = String::from_utf8(f.git(
        &f.task,
        &["hash-object", "-w", forged_file.to_str().unwrap()],
    ))
    .unwrap();
    // Build only a test-owned replacement commit, then restore the declared
    // positive fixture index before executing either comparator query.
    f.git(&f.task, &["read-tree", base]);
    f.git(
        &f.task,
        &[
            "update-index",
            "--cacheinfo",
            "100644",
            blob.trim(),
            manifest_path,
        ],
    );
    let tree = String::from_utf8(f.git(&f.task, &["write-tree"])).unwrap();
    let replacement = String::from_utf8(f.git(
        &f.task,
        &[
            "commit-tree",
            tree.trim(),
            "-m",
            "private replacement baseline",
        ],
    ))
    .unwrap();
    f.git(&f.task, &["read-tree", legitimate_tree.trim()]);
    f.git(&f.task, &["replace", base, replacement.trim()]);
    assert_eq!(
        f.git(&f.task, &["show", &format!("{base}:{manifest_path}")]),
        forged.as_bytes()
    );
    let bad_candidate = forged.replacen(manifest_before, manifest_after, 1);
    fs::write(f.task.join(manifest_path), &bad_candidate).unwrap();
    f.git(&f.task, &["add", "--", manifest_path]);
    let index = String::from_utf8(f.git(
        &f.task,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    ))
    .unwrap();
    let index = Path::new(index.trim());
    let index_before = fs::read(index).unwrap();
    let head_before = f.git(&f.task, &["rev-parse", "HEAD"]);
    assert!(
        std::panic::catch_unwind(|| assert_approved_test_dependency_change_at(
            &f.task,
            base,
            manifest_path
        ))
        .is_err()
    );
    assert_eq!(fs::read(index).unwrap(), index_before);
    assert_eq!(f.git(&f.task, &["rev-parse", "HEAD"]), head_before);
    assert_eq!(
        fs::read(f.task.join(manifest_path)).unwrap(),
        bad_candidate.as_bytes()
    );
    // A separate, explicitly prepared positive query must still succeed with
    // the same active replacement ref. This is not production rollback.
    fs::write(f.task.join(manifest_path), &legitimate_manifest).unwrap();
    f.git(&f.task, &["add", "--", manifest_path]);
    assert_approved_test_dependency_change_at(&f.task, base, manifest_path);
    assert_approved_test_dependency_change_at(&f.task, base, "Cargo.lock");
}

#[test]
#[ignore = "task-specific final-diff scope audit against the assigned initial HEAD"]
fn final_diff_contains_only_authorised_paths() {
    let history = complete_scope_history(&root(), "606f3c746e51b8ef66d75380b41791b9f3a9ed8e");
    let tmp = tempfile::tempdir().unwrap();
    let mut command = amplihack_git::command();
    isolated(&mut command, tmp.path());
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    let changed = checked(command.arg("-C").arg(root()).args([
        "diff",
        "--no-ext-diff",
        "--no-renames",
        "--name-only",
        "-z",
        "606f3c746e51b8ef66d75380b41791b9f3a9ed8e",
    ]))
    .stdout;
    let mut command = amplihack_git::command();
    isolated(&mut command, tmp.path());
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    let untracked = checked(command.arg("-C").arg(root()).args([
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
    ]))
    .stdout;
    let cached = scope_audit_git(&[
        "diff",
        "--cached",
        "--no-ext-diff",
        "--no-renames",
        "--name-only",
        "-z",
        "606f3c746e51b8ef66d75380b41791b9f3a9ed8e",
    ]);
    let mut history_paths = Vec::new();
    for revision in String::from_utf8(history).unwrap().lines() {
        history_paths.extend(scope_audit_git(&[
            "diff-tree",
            "--root",
            "-m",
            "--no-commit-id",
            "--name-only",
            "--no-ext-diff",
            "--no-renames",
            "-r",
            "-z",
            revision,
        ]));
    }
    let allowed_recipes: BTreeSet<_> = inventory()
        .iter()
        .filter(|c| c.authorised_edit)
        .map(|c| format!("amplifier-bundle/recipes/{}.yaml", c.asset))
        .collect();
    let complementary_tests = [
        "amplifier-bundle/recipes/tests/test-minimalist-auto-stage.py",
        "amplifier-bundle/recipes/tests/minimalist-auto-stage.md",
        "amplifier-bundle/recipes/tests/canonical-staging.md",
        "amplifier-bundle/recipes/tests/test-pragmatic-auto-stage.py",
        "amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py",
        "amplifier-bundle/recipes/tests/test-pragmatic-crusty-directories.py",
        "amplifier-bundle/recipes/tests/test-pragmatic-evidence.py",
        "amplifier-bundle/recipes/tests/canonical_staging_publication.py",
        "amplifier-bundle/recipes/tests/test-canonical-staging-harness.py",
    ];
    let delta_helpers = [
        "amplifier-bundle/recipes/hook-rejection-repair.yaml",
        "amplifier-bundle/tools/workflow_publish_checkpoint.sh",
        "amplifier-bundle/tools/workflow_review_checkpoint.sh",
        "amplifier-bundle/tools/workflow_cleanup_checkpoint.sh",
        "amplifier-bundle/tools/workflow_hook_repair.sh",
        "amplifier-bundle/tools/workflow_stage_changes.sh",
        "amplifier-bundle/tools/workflow_prepare_banner.sh",
        "amplifier-bundle/tools/autodrive_round_context.sh",
    ];
    let maintenance_paths: Vec<String> = serde_json::from_str(include_str!(
        "fixtures/canonical_staging/maintenance_paths.json"
    ))
    .expect("maintenance inventory must be a typed string array");
    let maintenance: BTreeSet<_> = maintenance_paths.iter().map(String::as_str).collect();
    assert_eq!(
        maintenance.len(),
        maintenance_paths.len(),
        "duplicate maintenance entries"
    );
    for path in changed
        .split(|b| *b == 0)
        .chain(cached.split(|b| *b == 0))
        .chain(history_paths.split(|b| *b == 0))
        .chain(untracked.split(|b| *b == 0))
        .filter(|p| !p.is_empty())
    {
        let path = std::str::from_utf8(path).expect("unexpected non-UTF-8 source change");
        let approved_dependency =
            matches!(path, "Cargo.lock" | "crates/amplihack-workflows/Cargo.toml");
        if approved_dependency {
            assert_approved_test_dependency_change(path);
        }
        assert!(
            approved_dependency
                || allowed_recipes.contains(path)
                || complementary_tests.contains(&path)
                || delta_helpers.contains(&path)
                || EXISTING_TASK_TEST_PATHS.contains(&path)
                || maintenance.contains(path),
            "unauthorised change: {path}; runner, consensus, unlisted helpers and all other dependencies must remain unchanged"
        );
    }
}

#[test]
#[ignore = "task-specific final-diff audit against the recorded initial asset hashes"]
fn bounded_recipe_changes_preserve_all_other_asset_bytes() {
    let hashes: std::collections::BTreeMap<String, String> = serde_json::from_str(include_str!(
        "fixtures/canonical_staging/baseline_asset_sha256.json"
    ))
    .unwrap();
    let tmp = tempfile::tempdir().unwrap();
    for (asset, expected) in hashes {
        let file = root().join(format!("amplifier-bundle/recipes/{asset}.yaml"));
        let mut text = fs::read_to_string(&file).unwrap();
        text = restore_auxiliary_extraction(&asset, text);
        text = restore_checkpoint_delta(&asset, text);
        if matches!(asset.as_str(), "workflow-pr-review" | "workflow-finalize") {
            let declaration = "  version_bump: \"\"\n";
            let parsed: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
            assert_eq!(parsed["context"]["version_bump"].as_str(), Some(""));
            assert_eq!(text.matches(declaration).count(), 1);
            text = text.replacen(declaration, "", 1);
        }
        if inventory()
            .iter()
            .any(|c| c.asset == asset && c.authorised_edit)
        {
            // Every baseline agent omits the field. Permit only the requested
            // added declaration, preserving prompts, commands and normal hooks.
            text = text
                .split_inclusive('\n')
                .filter(|line| *line != "    auto_stage: false\n")
                .collect();
        }
        let normalised = tmp.path().join(format!("{asset}.yaml"));
        fs::write(&normalised, text).unwrap();
        assert_eq!(
            sha256(&normalised),
            expected,
            "out-of-scope asset changes: {asset}"
        );
    }
}

#[test]
#[ignore = "requires parent-produced final-candidate evidence; absent, failed or mock gates must fail"]
fn final_candidate_delivery_requires_separate_complete_receipts() {
    let manifest = std::env::var_os("CANONICAL_STAGING_ACCEPTANCE_LEDGER")
        .expect("supply the real parent acceptance ledger; no synthetic ledger is accepted");
    let manifest = Path::new(&manifest).canonicalize().unwrap();
    let ledger_root = manifest.parent().unwrap();
    let ledger: Value = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let mut head_command = amplihack_git::command();
    isolated(&mut head_command, tmp.path());
    let head = checked(
        head_command
            .arg("-C")
            .arg(root())
            .args(["rev-parse", "HEAD"]),
    );
    let head = String::from_utf8(head.stdout).unwrap();
    assert_eq!(ledger["candidate_head"].as_str(), Some(head.trim()));
    let provenance = &ledger["provenance"];
    assert_eq!(
        provenance["origin"],
        "https://github.com/rysweet/amplihack-rs.git"
    );
    assert_eq!(provenance["branch"], "fix/canonical-default-staging-1554");
    assert_eq!(
        provenance["initial_head"],
        "606f3c746e51b8ef66d75380b41791b9f3a9ed8e"
    );
    assert_eq!(provenance["verification_before_source_analysis"], true);
    assert_eq!(
        provenance["implementation_directory"].as_str(),
        root().to_str()
    );
    assert!(
        provenance["caller_directory"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    assert!(
        provenance["runner_version"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
    );
    let baseline_hashes: std::collections::BTreeMap<String, String> = serde_json::from_str(
        include_str!("fixtures/canonical_staging/baseline_asset_sha256.json"),
    )
    .unwrap();
    let results = ledger["opt_outs"]
        .as_array()
        .expect("the full regression population is required");
    let cases = inventory();
    let required: Vec<_> = cases.iter().filter(|c| c.authorised_edit).collect();
    assert_eq!(
        results.len(),
        required.len(),
        "missing or extra per-step results"
    );
    let mut used = BTreeSet::new();
    for case in required {
        let result = results
            .iter()
            .find(|r| r["asset"] == case.asset && r["step_id"] == case.step_id)
            .expect("missing per-step baseline, GREEN or removal control");
        assert!(used.insert((case.asset.as_str(), case.step_id.as_str())));
        let mut projection_hashes = Vec::new();
        for variant in ["red", "green", "removal_control"] {
            let reference = &result[variant];
            let path = ledger_root
                .join(reference["path"].as_str().unwrap())
                .canonicalize()
                .unwrap();
            assert!(path.starts_with(ledger_root), "receipt escapes report root");
            assert_eq!(sha256(&path), reference["sha256"].as_str().unwrap());
            let receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(receipt["asset"], case.asset);
            assert_eq!(receipt["case_id"], case.step_id);
            assert_eq!(
                receipt["exit_status"], 0,
                "setup/runner failures cannot qualify as integrity RED"
            );
            assert_eq!(receipt["adapter_execution"]["completed"], true);
            let violations = receipt["violations"].as_array().unwrap();
            if variant == "green" {
                assert!(violations.is_empty());
                assert_eq!(receipt["raw_index_equal"], true);
                assert_eq!(receipt["semantic_index_equal"], true);
            } else {
                assert!(
                    violations.iter().any(|v| v
                        .as_str()
                        .is_some_and(|s| s.starts_with("index-integrity:"))),
                    "control failed before its intended integrity assertion"
                );
            }
            if variant == "red" {
                assert_eq!(
                    receipt["candidate_head"],
                    "606f3c746e51b8ef66d75380b41791b9f3a9ed8e"
                );
                assert_eq!(receipt["source_asset_sha256"], baseline_hashes[&case.asset]);
            } else {
                assert_eq!(receipt["candidate_head"], ledger["candidate_head"]);
                assert_eq!(
                    receipt["candidate_uncommitted_paths_nul"],
                    serde_json::json!([]),
                    "final-candidate evidence has uncommitted changes"
                );
                assert_eq!(
                    receipt["candidate_untracked_paths_nul"],
                    serde_json::json!([]),
                    "final-candidate evidence has untracked source files"
                );
                assert_eq!(
                    receipt["source_asset_sha256"],
                    sha256(&root().join(format!("amplifier-bundle/recipes/{}.yaml", case.asset)))
                );
                projection_hashes.push(receipt["projection_sha256"].clone());
            }
        }
        assert_ne!(
            projection_hashes[0], projection_hashes[1],
            "control did not remove its corresponding opt-out"
        );
    }
    // The original forty receipts above remain mandatory. The four new
    // opt-outs require their own real native GREEN and removal receipts.
    // Source checks and materialised-path diagnostics cannot fill this array.
    let additional = ledger["checkpoint_delta_opt_outs"]
        .as_array()
        .expect("four separately counted checkpoint-delta native controls are required");
    assert_eq!(additional.len(), 4);
    let mut delta_controls: Vec<_> = CHECKPOINT_DELTA
        .iter()
        .map(|entry| (entry.0, format!("{}-scope-review", entry.1)))
        .collect();
    delta_controls.push(("hook-rejection-repair", "fix-hook-rejection".into()));
    for (asset, id) in delta_controls {
        let result = additional
            .iter()
            .find(|row| row["asset"] == asset && row["step_id"] == id)
            .expect("missing additional opt-out control");
        let mut projections = Vec::new();
        for variant in ["green", "removal_control"] {
            let reference = &result[variant];
            let path = ledger_root
                .join(reference["path"].as_str().unwrap())
                .canonicalize()
                .unwrap();
            assert!(path.starts_with(ledger_root));
            assert_eq!(sha256(&path), reference["sha256"].as_str().unwrap());
            let receipt: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            assert_eq!(receipt["asset"], asset);
            assert_eq!(receipt["case_id"], id);
            assert_eq!(receipt["candidate_head"], ledger["candidate_head"]);
            assert_eq!(
                receipt["source_asset_sha256"],
                sha256(&root().join(format!("amplifier-bundle/recipes/{asset}.yaml")))
            );
            assert_eq!(receipt["exit_status"], 0);
            assert_eq!(receipt["adapter_execution"]["completed"], true);
            assert_eq!(receipt["materialised_working_dir"], false);
            let violations = receipt["violations"].as_array().unwrap();
            if variant == "green" {
                assert!(violations.is_empty());
                assert_eq!(receipt["raw_index_equal"], true);
                assert_eq!(receipt["semantic_index_equal"], true);
            } else {
                assert!(violations.iter().any(|value| {
                    value
                        .as_str()
                        .is_some_and(|s| s.starts_with("index-integrity:"))
                }));
            }
            projections.push(receipt["projection_sha256"].clone());
        }
        assert_ne!(projections[0], projections[1]);
    }
    for gate in [
        "consensus",
        "checkpoint_acceptance",
        "independent_review",
        "native_qualification",
        "proof_qualification",
        "normal_hooks",
        "full_qa",
        "ci",
        "actual_autodrive",
        "owner_integration",
    ] {
        let receipt = &ledger["gates"][gate];
        assert_eq!(
            receipt["status"], "passed",
            "missing or failed gate: {gate}"
        );
        assert_eq!(
            receipt["candidate_head"], ledger["candidate_head"],
            "stale gate: {gate}"
        );
        assert_eq!(
            receipt["mock"], false,
            "mock evidence cannot satisfy {gate}"
        );
        assert_eq!(
            receipt["exit_status"], 0,
            "terminal, budget and exit-79 failures cannot satisfy {gate}"
        );
        let artifacts = receipt["artifacts"]
            .as_array()
            .expect("gate must reference retained raw evidence");
        assert!(!artifacts.is_empty(), "missing raw evidence for {gate}");
        for artifact in artifacts {
            let path = ledger_root
                .join(artifact["path"].as_str().unwrap())
                .canonicalize()
                .unwrap();
            assert!(
                path.starts_with(ledger_root),
                "evidence path escapes the assigned report root"
            );
            assert_eq!(
                sha256(&path),
                artifact["sha256"].as_str().unwrap(),
                "stale evidence: {gate}"
            );
        }
    }
    let consensus = &ledger["gates"]["consensus"];
    assert_eq!(consensus["classification"], "critical");
    assert_eq!(consensus["workflow"], "consensus-workflow");
    assert_eq!(consensus["final_design_approved"], true);
    let completed: BTreeSet<_> = consensus["completed_steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    // Derive branch membership from the actual assets rather than accepting
    // orchestration SUCCESS as evidence that the #1966 route executed.
    for asset in ["consensus-tdd-impl", "consensus-pr-review"] {
        let source = recipe(asset);
        for s in source["steps"].as_sequence().unwrap() {
            if s["condition"].as_str() == Some("is_critical_code == 'true'")
                || (asset == "consensus-pr-review" && s.get("agent").is_some())
            {
                assert!(
                    completed.contains(s["id"].as_str().unwrap()),
                    "required consensus branch/review skipped: {}",
                    s["id"].as_str().unwrap()
                );
            }
        }
    }
    let auto = &ledger["gates"]["actual_autodrive"];
    assert_eq!(auto["platform"], "Linux");
    assert_eq!(auto["proc_available"], true);
    assert_eq!(auto["crusty_verdict"], "CLEAN");
    assert_eq!(auto["unresolved_concerns"], 0);
    assert_eq!(
        ledger["gates"]["independent_review"]["unresolved_threads"],
        0
    );
    assert_eq!(ledger["gates"]["independent_review"]["independent"], true);
    let checks = ledger["gates"]["ci"]["required_checks"].as_array().unwrap();
    assert!(!checks.is_empty(), "required CI inventory is missing");
    for check in checks {
        assert_eq!(check["status"], "completed");
        assert_eq!(check["conclusion"], "success");
    }
    for gate in [
        "native_qualification",
        "proof_qualification",
        "normal_hooks",
        "full_qa",
    ] {
        let r = &ledger["gates"][gate];
        assert!(r["command"].as_str().is_some_and(|s| !s.is_empty()));
        assert_eq!(r["memory_limit_bytes"], 4_u64 * 1024 * 1024 * 1024);
        assert!(
            r["resource_mechanism"]
                .as_str()
                .is_some_and(|s| !s.is_empty())
        );
        assert_eq!(r["nice"], 15);
        assert!(
            r["compilation_jobs"]
                .as_u64()
                .is_some_and(|v| (1..=2).contains(&v))
        );
        assert_eq!(r["test_parallelism"], "default");
        assert_eq!(r["resource_admission_resampled"], true);
        if gate.ends_with("qualification") {
            assert!(
                r["authorised_procedure_reference"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty())
            );
            assert_eq!(r["qualification_conditions"], "595+5");
        } else {
            assert_eq!(r["aggregate_timeout"], false);
            assert_eq!(r["naturally_completed"], true);
        }
    }
    let integration = &ledger["gates"]["owner_integration"];
    assert_eq!(integration["separately_authorised"], true);
    assert_eq!(integration["platform_state"], "MERGED");
    assert!(
        integration["platform_evidence_url"]
            .as_str()
            .is_some_and(|s| s.starts_with("https://"))
    );
    let issue_audit = &ledger["issue_audit"];
    assert_eq!(issue_audit["1554"]["state"], "OPEN");
    assert_eq!(issue_audit["1914"]["state"], "OPEN");
    for issue in ["1554", "1914"] {
        assert!(
            issue_audit[issue]["unresolved_requirements"]
                .as_array()
                .is_some_and(|a| !a.is_empty()),
            "the broader unresolved issue requirements must remain explicit"
        );
    }
    assert_eq!(
        issue_audit["references"],
        serde_json::json!(["Refs #1554", "Refs #1914"])
    );
}
