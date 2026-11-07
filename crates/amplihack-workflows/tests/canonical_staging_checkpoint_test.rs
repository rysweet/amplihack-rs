// File: crates/amplihack-workflows/tests/canonical_staging_checkpoint_test.rs
//! Unchanged checkpoint commands, actual helpers and normal Git hooks.
//! Violations remain failures; opt-outs cannot repair explicit broad staging.
#![cfg(unix)]
#[path = "support/canonical_staging.rs"]
mod support;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};
use std::path::{Path, PathBuf};
use support::*;

const CHECKPOINTS: &[(&str, &str)] = &[
    ("workflow-tdd", "checkpoint-after-implementation"),
    (
        "workflow-refactor-review",
        "checkpoint-after-review-feedback",
    ),
    ("workflow-publish", "step-15-commit-push"),
    ("workflow-pr-review", "step-18c-push-feedback-changes"),
    ("workflow-finalize", "step-20b-push-cleanup"),
    ("autodrive-crusty-round", "step-04-address-concerns"),
    ("workflow-publish", "step-16b-outside-in-fix-loop"),
];

fn staged_entries(bytes: &[u8]) -> BTreeMap<Vec<u8>, Vec<u8>> {
    bytes
        .split(|b| *b == 0)
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let tab = entry.iter().position(|b| *b == b'\t').unwrap();
            (entry[tab + 1..].to_vec(), entry[..tab].to_vec())
        })
        .collect()
}

fn commit_objects(f: &Fixture) -> BTreeSet<String> {
    String::from_utf8(f.git(
        &f.task,
        &[
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname) %(objecttype)",
        ],
    ))
    .unwrap()
    .lines()
    .filter_map(|line| line.strip_suffix(" commit").map(str::to_owned))
    .collect()
}

fn tree_entry(f: &Fixture, commit: &str, path: &str) -> serde_json::Value {
    tree_entry_at(f, &f.task, commit, path)
}

fn tree_entry_at(f: &Fixture, repository: &Path, commit: &str, path: &str) -> serde_json::Value {
    let raw = f.git(repository, &["ls-tree", "-z", commit, "--", path]);
    if raw.is_empty() {
        return serde_json::Value::Null;
    }
    let header = raw.split(|b| *b == b'\t').next().unwrap();
    let fields: Vec<_> = std::str::from_utf8(header).unwrap().split(' ').collect();
    let content = if fields[1] == "blob" {
        Some(f.git(repository, &["cat-file", "blob", fields[2]]))
    } else {
        None
    };
    json!({"mode":fields[0],"type":fields[1],"oid":fields[2],"content_bytes":content})
}

fn destination_observation(f: &Fixture, remote: &Path, expected_head: &[u8]) -> serde_json::Value {
    let tip =
        String::from_utf8(f.git(remote, &["rev-parse", "--verify", "refs/heads/task"])).unwrap();
    let tip = tip.trim();
    let source = tree_entry_at(f, remote, tip, "source.txt");
    let new_source = tree_entry_at(f, remote, tip, "new source.txt");
    let contents_correct = source["mode"] == "100644"
        && new_source["mode"] == "100644"
        && source["content_bytes"] == json!(b"authorised source change\n".as_slice())
        && new_source["content_bytes"] == json!(b"authorised new source\n".as_slice());
    let matches_head = tip.as_bytes() == expected_head.strip_suffix(b"\n").unwrap_or(expected_head);
    json!({"reference":"refs/heads/task","tip":tip,"source":source,"new_source":new_source,
           "matches_local_head":matches_head,"contents_authorised":contents_correct,
           "publication_observed":matches_head && contents_correct})
}

#[test]
#[ignore = "requires task-private evidence and temporary directories; no external tools beyond Git"]
fn actual_destination_evidence_detects_an_omitted_push() {
    let f = Fixture::checkpoint_clean("publication-destination-oracle");
    let remote = f.temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    f.git(&remote, &["init", "--bare", "--template="]);
    f.git(
        &f.task,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    f.git(&f.task, &["push", "-u", "origin", "task"]);
    fs::write(f.task.join("source.txt"), "authorised source change\n").unwrap();
    fs::write(f.task.join("new source.txt"), "authorised new source\n").unwrap();
    f.git(&f.task, &["add", "--", "source.txt", "new source.txt"]);
    f.git(
        &f.task,
        &["commit", "-m", "local source without publication"],
    );
    let head = f.git(&f.task, &["rev-parse", "HEAD"]);
    let missing = destination_observation(&f, &remote, &head);
    assert_eq!(missing["publication_observed"], false);
    assert_eq!(missing["matches_local_head"], false);
    f.git(&f.task, &["push", "origin", "task"]);
    let published = destination_observation(&f, &remote, &head);
    assert_eq!(published["publication_observed"], true);
    assert_eq!(published["contents_authorised"], true);
}

fn publication_history(
    f: &Fixture,
    before: &BTreeSet<String>,
) -> (Vec<serde_json::Value>, Vec<String>, bool) {
    let commits: Vec<_> = commit_objects(f).difference(before).cloned().collect();
    let mut records = Vec::new();
    for commit in &commits {
        let parents =
            String::from_utf8(f.git(&f.task, &["rev-list", "--parents", "-n", "1", commit]))
                .unwrap();
        let parents: Vec<_> = parents.split_whitespace().skip(1).collect();
        let parents = if parents.is_empty() {
            vec![None]
        } else {
            parents.into_iter().map(Some).collect()
        };
        for parent in parents {
            let paths = if let Some(parent) = parent {
                f.git(
                    &f.task,
                    &["diff", "--name-only", "--no-renames", "-z", parent, commit],
                )
            } else {
                f.git(
                    &f.task,
                    &[
                        "diff-tree",
                        "--root",
                        "--no-commit-id",
                        "--name-only",
                        "--no-renames",
                        "-r",
                        "-z",
                        commit,
                    ],
                )
            };
            for path in paths.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                let name = std::str::from_utf8(path).unwrap();
                let old = parent
                    .map(|p| tree_entry(f, p, name))
                    .unwrap_or(serde_json::Value::Null);
                let new = tree_entry(f, commit, name);
                let expected: Option<&[u8]> = match path {
                    b"source.txt" => Some(b"authorised source change\n"),
                    b"new source.txt" => Some(b"authorised new source\n"),
                    _ => None,
                };
                let allowed = expected.is_some()
                    && new["mode"] == "100644"
                    && new["type"] == "blob"
                    && new["content_bytes"] == json!(expected);
                records.push(json!({"commit":commit,"parent":parent,"path_bytes":path,
                    "before":old,"after":new,"authorised":allowed}));
            }
        }
    }
    let allowed = records.iter().all(|r| r["authorised"] == true);
    (records, commits, allowed)
}

#[test]
#[ignore = "requires task-private evidence and temporary directories; no external tools beyond Git"]
fn publication_controls_retain_deleted_content_modes_and_unreachable_commits() {
    let f = Fixture::checkpoint("publication-controls");
    f.git(&f.task, &["reset", "--hard", "HEAD"]);
    let before = commit_objects(&f);
    fs::create_dir_all(f.task.join(".onboarding-evidence")).unwrap();
    fs::write(
        f.task.join(".onboarding-evidence/private.json"),
        "private\n",
    )
    .unwrap();
    f.git(&f.task, &["add", "-A"]);
    f.git(&f.task, &["commit", "-m", "publish forbidden content"]);
    f.git(&f.task, &["rm", "--", ".onboarding-evidence/private.json"]);
    f.git(&f.task, &["commit", "-m", "delete forbidden content"]);
    let (records, commits, allowed) = publication_history(&f, &before);
    assert_eq!(commits.len(), 2);
    assert!(!allowed);
    assert!(
        records.iter().any(|r| r["after"].is_null()
            && r["before"]["content_bytes"] == json!(b"private\n".as_slice()))
    );
    let before = commit_objects(&f);
    fs::write(f.task.join("source.txt"), "authorised source change\n").unwrap();
    f.git(&f.task, &["add", "--", "source.txt"]);
    f.git(&f.task, &["update-index", "--chmod=+x", "--", "source.txt"]);
    f.git(&f.task, &["commit", "-m", "forbidden executable mode"]);
    let bad_commit = f.git(&f.task, &["rev-parse", "HEAD"]);
    f.git(&f.task, &["reset", "--hard", "HEAD~1"]);
    let (records, commits, allowed) = publication_history(&f, &before);
    assert!(!allowed);
    assert_eq!(
        commits,
        vec![String::from_utf8(bad_commit).unwrap().trim().to_string()]
    );
    assert_eq!(records[0]["after"]["mode"], "100755");
}

fn worktree_state(root: &Path) -> BTreeMap<Vec<u8>, (u32, u8, Vec<u8>)> {
    fn visit(root: &Path, directory: &Path, result: &mut BTreeMap<Vec<u8>, (u32, u8, Vec<u8>)>) {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            let info = fs::symlink_metadata(&path).unwrap();
            if directory == root && path.file_name().unwrap() == ".git" && info.is_dir() {
                continue;
            }
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .as_os_str()
                .as_bytes()
                .to_vec();
            let mode = info.permissions().mode();
            if info.file_type().is_symlink() {
                result.insert(
                    relative,
                    (
                        mode,
                        1,
                        fs::read_link(&path)
                            .unwrap()
                            .as_os_str()
                            .as_bytes()
                            .to_vec(),
                    ),
                );
            } else if info.is_dir() {
                result.insert(relative, (mode, 2, Vec::new()));
                visit(root, &path, result);
            } else {
                assert!(info.is_file(), "unsupported fixture object");
                result.insert(relative, (mode, 3, fs::read(path).unwrap()));
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn checkpoint(
    asset: &str,
    id: &str,
    reject: bool,
    adversarial: bool,
    runtime_metadata: bool,
) -> (Fixture, serde_json::Value) {
    let f = if adversarial {
        Fixture::checkpoint(id)
    } else {
        Fixture::checkpoint_clean(id)
    };
    let owner_task = "fixture owner: change only literal source.txt and new source.txt";
    let binary = PathBuf::from(
        std::env::var_os("CANONICAL_STAGING_AMPLIHACK")
            .expect("set CANONICAL_STAGING_AMPLIHACK to the authorised actual executable"),
    );
    assert!(binary.is_absolute() && binary.is_file());
    // Only the launch boundary for a repair agent is refused. Hygiene and all
    // deterministic helpers are real; no helper is replaced or reimplemented.
    let bin = f.temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    executable(
        &bin.join("amplihack"),
        "#!/bin/bash\nif [ \"${1:-}\" = recipe ]; then\n  printf 'repair-agent-refused\\n' >>\"$AMPLIHACK_STAGING_REPAIR_TRACE\"\n  echo 'BLOCKED_TERMINAL orchestration_unavailable' >&2\n  exit 79\nfi\nexec \"$AMPLIHACK_STAGING_REAL_BINARY\" \"$@\"\n",
    );
    // The actual Crusty guard calls this exact provider query. No other
    // provider operation is permitted and no credentials are available.
    executable(
        &bin.join("gh"),
        "#!/bin/bash\nif [ \"$*\" = 'pr view 1554 --json headRefName --jq .headRefName' ]; then\n  printf 'task\\n'\nelse\n  echo 'unexpected provider operation' >&2\n  exit 79\nfi\n",
    );
    let remote = f.temp.path().join("remote.git");
    fs::create_dir(&remote).unwrap();
    f.git(&remote, &["init", "--bare", "--template="]);
    f.git(
        &f.review,
        &["remote", "add", "origin", remote.to_str().unwrap()],
    );
    f.git(&f.task, &["push", "-u", "origin", "task"]);
    let remote_before = f.git(&remote, &["rev-parse", "refs/heads/task"]);
    let hook_log = f.evidence.join("hook-invocations.jsonl");
    let hooks = f.temp.path().join("hooks");
    fs::create_dir(&hooks).unwrap();
    f.git(
        &f.task,
        &["config", "core.hooksPath", hooks.to_str().unwrap()],
    );
    executable(
        &hooks.join("pre-commit"),
        if reject {
            "#!/bin/bash\nprintf 'rejecting-pre-commit\\n' >>\"$AMPLIHACK_STAGING_HOOK_LOG\"\ngit ls-files --stage -z >\"$AMPLIHACK_STAGING_HOOK_INDEX\"\nexit 1\n"
        } else {
            "#!/bin/bash\nprintf 'successful-pre-commit\\n' >>\"$AMPLIHACK_STAGING_HOOK_LOG\"\ngit ls-files --stage -z >\"$AMPLIHACK_STAGING_HOOK_INDEX\"\nexit 0\n"
        },
    );
    fs::write(f.task.join("source.txt"), "authorised source change\n").unwrap();
    fs::write(f.task.join("new source.txt"), "authorised new source\n").unwrap();
    if runtime_metadata && !adversarial {
        fs::create_dir_all(f.task.join(".amplihack")).unwrap();
        fs::write(f.task.join(".amplihack/indexing.pid"), "4242\n").unwrap();
    }
    if adversarial {
        fs::create_dir_all(f.task.join(".amplihack")).unwrap();
        fs::write(f.task.join(".amplihack/indexing.pid"), "4242\n").unwrap();
        // This path is not removed by runtime cleanup. Its exclusion must
        // result from staging policy, not disappearance or ignore rules.
        fs::create_dir_all(f.task.join(".onboarding-evidence")).unwrap();
        fs::write(f.task.join(".onboarding-evidence/checkpoint.json"), "{}\n").unwrap();
    }
    let document = recipe(asset);
    let selected = document["steps"]
        .as_sequence()
        .unwrap()
        .iter()
        .find(|s| s["id"].as_str() == Some(id))
        .unwrap();
    let raw_block = if let Some(command) = selected["command"].as_str() {
        command.to_owned()
    } else {
        let prompt = selected["prompt"].as_str().unwrap();
        // Mechanically extract the complete existing commit fence; do not
        // rewrite its shell options, staging, error handling or hook commands.
        prompt
            .split_once("```bash\n")
            .expect("missing executable commit fence")
            .1
            .split_once("```")
            .expect("unterminated executable commit fence")
            .0
            .to_owned()
    };
    let block = raw_block
        .replace("{{repo_path}}", f.task.to_str().unwrap())
        .replace(
            "{{autodrive_launch_tools}}",
            root().join("amplifier-bundle/tools").to_str().unwrap(),
        )
        .replace("{{pr_number}}", "1554");
    let original = f.evidence.join("source-block.sh");
    fs::write(&original, &raw_block).unwrap();
    let script = f.evidence.join("unchanged-checkpoint.sh");
    fs::write(&script, &block).unwrap();
    let before_head = f.git(&f.task, &["rev-parse", "HEAD"]);
    let all_heads_before: Vec<_> = [&f.caller, &f.review, &f.task]
        .iter()
        .map(|repo| f.git(repo, &["rev-parse", "HEAD"]))
        .collect();
    let paths = f.indexes();
    let before_objects = commit_objects(&f);
    let before_raw = f.raw(&paths, "checkpoint-before");
    let entries_before = f.entries(&f.task);
    let user_blob_before = f.git(&f.task, &["show", ":user.txt"]);
    let user_mode_before = f.git(&f.task, &["ls-files", "--stage", "--", "user.txt"]);
    let task_worktree_before = worktree_state(&f.task);
    let invocation_dir = if selected.get("agent").is_some() {
        &f.task
    } else {
        &f.caller
    };
    let mut cmd = f.command("bash", invocation_dir);
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("CANONICAL_STAGING_TOOL_PATH")
            .unwrap_or_else(|_| "/usr/bin:/bin:/usr/sbin:/sbin:/opt/homebrew/bin".into())
    );
    let bounded = matches!(
        (asset, id),
        ("workflow-publish", "step-15-commit-push")
            | ("workflow-pr-review", "step-18c-push-feedback-changes")
            | ("workflow-finalize", "step-20b-push-cleanup")
    );
    if bounded {
        let request = f.evidence.join("scope-projection-request.json");
        fs::write(
            &request,
            serde_json::to_vec_pretty(&json!({
                "asset":asset, "fixture_owner_paths":["source.txt","new source.txt"],
                "evidence_directory":f.evidence, "shell":"bash",
                "context":{"repo_path":f.review, "task_description":owner_task,
                    "final_requirements":owner_task, "pr_number":"1554",
                    "version_bump":{"change_classification":"PATCH"},
                    "worktree_setup":{"worktree_path":f.task,"branch_name":"task",
                        "base_ref":"origin/task","base_branch":"main","created":true}}
            }))
            .unwrap(),
        )
        .unwrap();
        cmd = f.command("python3", &f.task);
        cmd.arg(root().join("amplifier-bundle/recipes/tests/test-pragmatic-checkpoints.py"))
            .arg("--bounded-fixture-projection")
            .arg(&request);
    } else {
        cmd.arg(&script);
    }
    cmd.env("PATH", path)
        .env("AMPLIHACK_HOME", root())
        .env("REPO_PATH", &f.review)
        .env(
            "RECIPE_VAR_worktree_setup",
            json!({"worktree_path":f.task,"branch_name":"task"}).to_string(),
        )
        .env(
            "RECIPE_VAR_version_bump",
            "{\"change_classification\":\"PATCH\"}",
        )
        .env("TASK_DESCRIPTION", owner_task)
        .env("ISSUE_NUMBER", "1554")
        .env("REMOTE_HOST_TYPE", "other")
        .env("AMPLIHACK_STAGING_REAL_BINARY", &binary)
        .env(
            "AMPLIHACK_STAGING_REPAIR_TRACE",
            f.evidence.join("repair-refusal.log"),
        )
        .env("AMPLIHACK_STAGING_HOOK_LOG", &hook_log)
        .env(
            "AMPLIHACK_STAGING_HOOK_INDEX",
            f.evidence.join("hook.index-entries"),
        )
        .env("AMPLIHACK_GIT_AUTHOR_NAME", "Fixture Author")
        .env("AMPLIHACK_GIT_AUTHOR_EMAIL", "fixture@example.com")
        .env("AMPLIHACK_GIT_COMMITTER_NAME", "Fixture Author")
        .env("AMPLIHACK_GIT_COMMITTER_EMAIL", "fixture@example.com");
    let invocation = format!("{cmd:?}");
    fs::write(
        f.evidence.join("execution.json"),
        serde_json::to_vec_pretty(&json!({"command":invocation,"state":"running"})).unwrap(),
    )
    .unwrap();
    let out = cmd.output().unwrap();
    let after_raw = f.raw(&paths, "checkpoint-after"); // Before post-run Git diagnostics.
    fs::write(f.evidence.join("checkpoint.stdout"), &out.stdout).unwrap();
    fs::write(f.evidence.join("checkpoint.stderr"), &out.stderr).unwrap();
    fs::write(
        f.evidence.join("execution.json"),
        serde_json::to_vec_pretty(
            &json!({"command":invocation,"state":"returned","exit_status":out.status.code()}),
        )
        .unwrap(),
    )
    .unwrap();
    // This failed diagnostic receipt survives even if a later Git read panics.
    fs::write(f.evidence.join("checkpoint-receipt.json"), serde_json::to_vec_pretty(&json!({
        "finalized":true,"asset":asset,"step_id":id,"command":invocation,"exit_status":out.status.code(),
        "accepted":false,"failure_phase":"checkpoint_diagnostics_not_completed"
    })).unwrap()).unwrap();
    let after_head = f.git(&f.task, &["rev-parse", "HEAD"]);
    let committed = before_head != after_head;
    let destination = destination_observation(&f, &remote, &after_head);
    let push_required = !matches!(asset, "workflow-tdd" | "workflow-refactor-review");
    let remote_unchanged = destination["tip"] == String::from_utf8_lossy(&remote_before).trim();
    let destination_contract_satisfied = if push_required && !reject && !adversarial {
        destination["publication_observed"] == true && !remote_unchanged
    } else {
        remote_unchanged
    };
    let (published_contents, created_commits, every_published_content_authorised) =
        publication_history(&f, &before_objects);
    let mut changes = Vec::new();
    for row in &published_contents {
        for byte in row["path_bytes"].as_array().unwrap() {
            changes.push(byte.as_u64().unwrap() as u8);
        }
        changes.push(0);
    }
    let authorised = [b"source.txt".as_slice(), b"new source.txt".as_slice()];
    let published_only_authorised = committed && every_published_content_authorised;
    // Compare the actual pre-commit index, where available, independently of
    // the resulting commit. Existing user entries confer no publication right.
    let after_entries = f.entries(&f.task);
    let hook_entries = fs::read(f.evidence.join("hook.index-entries")).ok();
    let observed_entries = hook_entries.as_deref().unwrap_or(&after_entries);
    let before_entries = staged_entries(&entries_before);
    let observed = staged_entries(observed_entries);
    let newly_staged: Vec<_> = observed
        .iter()
        .filter(|(path, entry)| before_entries.get(*path) != Some(*entry))
        .map(|(path, _)| path.clone())
        .collect();
    let staging_only_authorised = newly_staged.len() == 2
        && newly_staged
            .iter()
            .all(|p| authorised.contains(&p.as_slice()))
        && before_entries
            .iter()
            .all(|(p, entry)| authorised.contains(&p.as_slice()) || observed.get(p) == Some(entry));
    let user_entry = f.git(&f.task, &["show", ":user.txt"]);
    let mode = f.git(&f.task, &["ls-files", "--stage", "--", "user.txt"]);
    let hooks_ran = fs::read_to_string(&hook_log).unwrap_or_default();
    fs::write(f.evidence.join("checkpoint.stdout"), &out.stdout).unwrap();
    fs::write(f.evidence.join("checkpoint.stderr"), &out.stderr).unwrap();
    fs::write(f.evidence.join("entries.before"), &entries_before).unwrap();
    fs::write(f.evidence.join("entries.after"), &after_entries).unwrap();
    fs::write(f.evidence.join("committed-paths.nul"), changes).unwrap();
    let mut receipt = json!({"finalized":true,"asset":asset,"step_id":id,"rejecting_hook":reject,"adversarial":adversarial,
        "candidate_head":String::from_utf8(f.git(&root(), &["rev-parse","HEAD"])).unwrap().trim(),
        "asset_sha256":sha256(&root().join(format!("amplifier-bundle/recipes/{asset}.yaml"))),
        "extracted_block_sha256":sha256(&original),"interpolated_block_sha256":sha256(&script),"command":invocation,"exit_status":out.status.code(),
        "actual_binary":binary,"actual_binary_sha256":executable_digest(&binary),"hook_evidence":hooks_ran,
        "before_head":String::from_utf8(before_head).unwrap().trim(),"after_head":String::from_utf8(after_head).unwrap().trim(),
        "commit_created":!created_commits.is_empty(),"created_commits":created_commits,"published_only_authorised":published_only_authorised,
        "newly_staged_paths_bytes":newly_staged,"staging_only_authorised":staging_only_authorised,
        "staging_observation":if hook_entries.is_some(){"pre-commit hook snapshot"}else{"post-checkpoint index; hook not reached"},
        "every_published_content_authorised":every_published_content_authorised,"published_contents":published_contents,
        "allowed_staging_paths":["source.txt","new source.txt"],"allowed_commit_paths":["source.txt","new source.txt"],
        "allowed_contents":{"source.txt":"authorised source change\n","new source.txt":"authorised new source\n"},
        "user_staged_blob_preserved":user_entry==user_blob_before,
        "user_staged_mode_preserved":mode.get(..6)==user_mode_before.get(..6),
        "task_worktree_equal":worktree_state(&f.task)==task_worktree_before,
        "all_heads_preserved":all_heads_before==[&f.caller, &f.review, &f.task].iter()
            .map(|repo| f.git(repo, &["rev-parse", "HEAD"])).collect::<Vec<_>>(),
        "fixture_kind":if adversarial{"unrelated partial staging and unknown metadata"}else{"independent clean task worktree"},
        "runtime_metadata_created":runtime_metadata,
        "caller_index_equal":before_raw[&f.caller]==after_raw[&f.caller],
        "review_index_equal":before_raw[&f.review]==after_raw[&f.review],
        "task_raw_index_equal":before_raw[&f.task]==after_raw[&f.task],
        "metadata_staged":!f.git(&f.task,&["ls-files","-z","--",".amplihack/indexing.pid",".onboarding-evidence/checkpoint.json"]).is_empty(),
        "source_commit_contents_correct":committed
            && tree_entry(&f,"HEAD","source.txt")["content_bytes"]==json!(b"authorised source change\n".as_slice())
            && tree_entry(&f,"HEAD","new source.txt")["content_bytes"]==json!(b"authorised new source\n".as_slice()),
        "residual_differences":["local bare remote; controlled fixture hooks; actual repair-agent launch refused with exit 79; no preceding agents; private gh answers only commit-dir branch lookup; prompt fences execute once with documented literal interpolation"]});
    receipt["push_required"] = json!(push_required);
    receipt["destination_contract_satisfied"] = json!(destination_contract_satisfied);
    receipt["destination_tip_before"] = json!(String::from_utf8_lossy(&remote_before).trim());
    receipt["destination_observation"] = destination;
    fs::write(
        f.evidence.join("checkpoint-receipt.json"),
        serde_json::to_vec_pretty(&receipt).unwrap(),
    )
    .unwrap();
    (f, receipt)
}

#[test]
#[ignore = "requires actual amplihack and helpers; expected to expose separately owned checkpoint defects"]
fn unchanged_checkpoints_stage_and_publish_only_authorised_source() {
    let mut failures = Vec::new();
    for (asset, id) in CHECKPOINTS {
        let (f, r) = checkpoint(asset, id, false, false, true);
        for key in [
            "published_only_authorised",
            "destination_contract_satisfied",
            "every_published_content_authorised",
            "staging_only_authorised",
            "user_staged_blob_preserved",
            "user_staged_mode_preserved",
            "caller_index_equal",
            "review_index_equal",
        ] {
            if r[key] != true {
                failures.push(format!("{id}: {key}; {}", f.evidence.display()));
            }
        }
        if r["metadata_staged"] != false
            || r["exit_status"] != 0
            || r["hook_evidence"] != "successful-pre-commit\n"
        {
            failures.push(format!(
                "{id}: metadata, hook or checkpoint failure; {}",
                f.evidence.display()
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "failed checkpoint acceptance; parent sequencing required:\n{}",
        failures.join("\n")
    );
}

#[test]
#[ignore = "requires actual helpers; no real repair model is invoked"]
fn rejecting_hooks_prevent_commits_and_propagate_checkpoint_failure() {
    let mut failures = Vec::new();
    for (asset, id) in CHECKPOINTS {
        // Omit new metadata here so an unrelated artefact guard cannot keep
        // this case from reaching its intended rejecting-hook assertion.
        let (f, r) = checkpoint(asset, id, true, false, false);
        if r["hook_evidence"] != "rejecting-pre-commit\n"
            || r["destination_contract_satisfied"] != true
            || r["commit_created"] != false
            || r["exit_status"] == 0
        {
            failures.push(format!(
                "{id}: rejecting hook not enforced; {}",
                f.evidence.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "requires actual amplihack; verifies successful hook execution independently of metadata rejection"]
fn successful_hooks_execute_in_unchanged_checkpoint_blocks() {
    let mut failures = Vec::new();
    for (asset, id) in CHECKPOINTS {
        let (f, r) = checkpoint(asset, id, false, false, false);
        if r["hook_evidence"] != "successful-pre-commit\n"
            || r["destination_contract_satisfied"] != true
            || r["commit_created"] != true
            || r["source_commit_contents_correct"] != true
            || r["exit_status"] != 0
        {
            failures.push(format!(
                "{id}: success-path hook or source commit failed; {}",
                f.evidence.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "requires actual helpers; retains all seven unauthorised-entry controls"]
fn all_seven_checkpoints_refuse_unrelated_staging_before_mutation() {
    let mut failures = Vec::new();
    for (asset, id) in CHECKPOINTS {
        let (f, r) = checkpoint(asset, id, false, true, false);
        for key in [
            "destination_contract_satisfied",
            "task_raw_index_equal",
            "task_worktree_equal",
            "all_heads_preserved",
            "user_staged_blob_preserved",
            "user_staged_mode_preserved",
            "caller_index_equal",
            "review_index_equal",
        ] {
            if r[key] != true {
                failures.push(format!("{id}: {key}; {}", f.evidence.display()));
            }
        }
        if r["exit_status"] == 0 || r["commit_created"] != false || r["hook_evidence"] != "" {
            failures.push(format!(
                "{id}: unauthorised entry was not refused before hooks; {}",
                f.evidence.display()
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
#[ignore = "publication acceptance: no later recipe checkpoint; agent publication authority is unverified"]
fn mergeability_repairs_have_an_explicit_source_publication_owner() {
    let phase = recipe("workflow-finalize");
    let steps = phase["steps"].as_sequence().unwrap();
    let position = steps
        .iter()
        .position(|s| s["id"].as_str() == Some("step-22-ensure-mergeable"))
        .unwrap();
    let owns = steps[position..].iter().any(|s| {
        s["command"]
            .as_str()
            .or_else(|| s["prompt"].as_str())
            .is_some_and(|text| text.contains("git commit "))
    });
    assert!(
        owns,
        "step-22 has no later recipe source checkpoint; the CI agent illustrates commit/push, but its directory and publication authority remain unverified and require parent sequencing"
    );
}
