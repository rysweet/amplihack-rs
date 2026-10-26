//! tests/integration/artifact_guard_workflow_tests.rs
//!
//! Structural contracts for issue #755 workflow integration.
//!
//! Every recipe that performs broad staging must invoke Artifact Guard first so
//! generated/runtime artifacts cannot be swept into a default workflow commit
//! or PR publication path.

use serde_yaml::Value;
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn recipes_dir() -> PathBuf {
    workspace_root().join("amplifier-bundle").join("recipes")
}

fn recipe_path(name: &str) -> PathBuf {
    recipes_dir().join(name)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn load_recipe(name: &str) -> Value {
    let path = recipe_path(name);
    serde_yaml::from_str(&read(&path)).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

fn steps(recipe: &Value) -> &[Value] {
    recipe
        .get("steps")
        .and_then(Value::as_sequence)
        .expect("recipe must contain top-level steps")
}

fn step_index(recipe: &Value, id: &str) -> usize {
    steps(recipe)
        .iter()
        .position(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .unwrap_or_else(|| panic!("missing recipe step `{id}`"))
}

fn step_text<'a>(recipe: &'a Value, id: &str, field: &str) -> &'a str {
    steps(recipe)
        .iter()
        .find(|step| step.get("id").and_then(Value::as_str) == Some(id))
        .and_then(|step| step.get(field).and_then(Value::as_str))
        .unwrap_or_else(|| panic!("step `{id}` must contain a {field}"))
}

fn recipe_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(recipes_dir()).expect("read recipes dir") {
        let path = entry.expect("recipe dir entry").path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("yaml") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn publish_runs_artifact_guard_before_commit_push_and_pr_publication_paths() {
    let recipe = load_recipe("workflow-publish.yaml");
    let commit_index = step_index(&recipe, "step-15-commit-push");
    let publish_index = step_index(&recipe, "step-16-create-draft-pr");
    let fix_loop_index = step_index(&recipe, "step-16b-outside-in-fix-loop");

    let guard_index = steps(&recipe)
        .iter()
        .position(|step| {
            step.get("command")
                .and_then(Value::as_str)
                .is_some_and(|command| {
                    command.contains("amplihack hygiene artifact-guard")
                        && command.contains("--mode pre-publish")
                })
        })
        .expect("workflow-publish must include an artifact guard command step");

    assert!(
        guard_index < commit_index && guard_index < publish_index && guard_index < fix_loop_index,
        "artifact guard must run before broad staging, PR creation, and outside-in fix publication"
    );
}

#[test]
fn finalize_runs_artifact_guard_before_cleanup_commit_or_final_push() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let final_cleanup_index = step_index(&recipe, "step-20-final-cleanup");
    let guard_index = step_index(&recipe, "step-20a-artifact-guard");
    let push_cleanup_index = step_index(&recipe, "step-20b-push-cleanup");
    let guard_command = step_text(&recipe, "step-20a-artifact-guard", "command");

    assert!(
        guard_command.contains("amplihack hygiene artifact-guard")
            && guard_command.contains("--mode pre-publish"),
        "workflow-finalize step-20a must invoke Artifact Guard in pre-publish mode"
    );
    assert!(
        final_cleanup_index < guard_index && guard_index < push_cleanup_index,
        "artifact guard must run after agent cleanup and before workflow-finalize cleanup commit/push"
    );
}

#[test]
fn finalize_push_cleanup_guards_immediately_before_broad_staging() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let command = step_text(&recipe, "step-20b-push-cleanup", "command");
    let guard = command
        .find("amplihack hygiene artifact-guard --repo . --mode pre-publish")
        .expect("step-20b must guard the exact repo before broad staging");
    let add = command
        .find("git add -A")
        .expect("step-20b currently performs broad staging");
    let between = &command[guard..add];

    assert!(
        guard < add,
        "step-20b must invoke Artifact Guard before git add -A"
    );
    assert!(
        between
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.is_empty()
                    && !trimmed.starts_with("amplihack hygiene artifact-guard")
                    && !trimmed.starts_with("git add -A")
            })
            .count()
            == 0,
        "Artifact Guard must be the deterministic final gate immediately before git add -A; intervening lines:\n{between}"
    );
}

#[test]
fn finalize_artifact_guard_failures_are_not_silenced() {
    let recipe = load_recipe("workflow-finalize.yaml");
    let guard_commands: Vec<&str> = steps(&recipe)
        .iter()
        .filter_map(|step| step.get("command").and_then(Value::as_str))
        .filter(|command| command.contains("amplihack hygiene artifact-guard"))
        .collect();

    assert!(
        !guard_commands.is_empty(),
        "workflow-finalize must invoke Artifact Guard"
    );
    for command in guard_commands {
        for line in command
            .lines()
            .filter(|line| line.contains("amplihack hygiene artifact-guard"))
        {
            assert!(
                !line.contains("|| true")
                    && !line.contains("|| :")
                    && !line.contains("2>/dev/null"),
                "Artifact Guard failures must fail loudly, not be hidden: `{line}`"
            );
        }
    }
}

#[test]
fn every_recipe_with_git_add_all_invokes_guard_first() {
    let mut missing = Vec::new();

    for path in recipe_files() {
        let text = read(&path);
        let broad_add_positions: Vec<usize> = ["git add -A", "git add --all", "git add ."]
            .iter()
            .flat_map(|needle| text.match_indices(needle).map(|(index, _)| index))
            .collect();
        if broad_add_positions.is_empty() {
            continue;
        }

        let guard_position = text.find("amplihack hygiene artifact-guard");
        for add_position in broad_add_positions {
            if guard_position.is_none_or(|guard| guard >= add_position) {
                missing.push(format!(
                    "{}: missing artifact guard before broad staging at byte {add_position}",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(&path)
                        .display()
                ));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "all broad staging recipes must run Artifact Guard first:\n{}",
        missing.join("\n")
    );
}

#[test]
fn recipe_commands_with_guarded_broad_staging_fail_loudly() {
    let mut unsafe_commands = Vec::new();

    for path in recipe_files() {
        let recipe = serde_yaml::from_str::<Value>(&read(&path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        for step in steps(&recipe) {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let Some(command) = step.get("command").and_then(Value::as_str) else {
                continue;
            };
            let Some(guard_position) = command.find("amplihack hygiene artifact-guard") else {
                continue;
            };
            let broad_add_after_guard =
                ["git add -A", "git add --all", "git add ."]
                    .iter()
                    .any(|needle| {
                        command
                            .match_indices(needle)
                            .any(|(add_position, _)| guard_position < add_position)
                    });
            if broad_add_after_guard && !command.contains("set -euo pipefail") {
                unsafe_commands.push(format!(
                    "{}:{id} guards broad staging but lacks `set -euo pipefail`",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(&path)
                        .display()
                ));
            }
        }
    }

    assert!(
        unsafe_commands.is_empty(),
        "guard failures must stop broad staging:\n{}",
        unsafe_commands.join("\n")
    );
}

#[test]
fn publish_and_finalize_do_not_inline_shell_delete_artifacts_as_remediation() {
    for recipe_name in ["workflow-publish.yaml", "workflow-finalize.yaml"] {
        let text = read(&recipe_path(recipe_name));
        for forbidden in [
            "rm -rf node_modules",
            "rm -rf dist",
            "rm -rf .claude/runtime",
            "git clean -fdx",
            "git reset --hard",
        ] {
            assert!(
                !text.contains(forbidden),
                "{recipe_name} must fail with remediation, not silently delete artifacts via `{forbidden}`"
            );
        }
    }
}

#[test]
fn guard_command_uses_structured_cli_arguments_not_shell_interpolation() {
    for recipe_name in ["workflow-publish.yaml", "workflow-finalize.yaml"] {
        let recipe = load_recipe(recipe_name);
        let commands: Vec<&str> = steps(&recipe)
            .iter()
            .filter_map(|step| step.get("command").and_then(Value::as_str))
            .filter(|command| command.contains("amplihack hygiene artifact-guard"))
            .collect();

        assert!(
            !commands.is_empty(),
            "{recipe_name} must invoke amplihack hygiene artifact-guard"
        );
        for command in commands {
            assert!(
                command.contains("--repo .") || command.contains("--repo \"$REPO_PATH\""),
                "{recipe_name} guard must pass an explicit repo path; command was:\n{command}"
            );
            assert!(
                command.contains("--mode pre-publish"),
                "{recipe_name} guard must use pre-publish mode before PR/finalize publication; command was:\n{command}"
            );
            assert!(
                !command.contains("eval ") && !command.contains("sh -c"),
                "{recipe_name} guard must not be invoked through eval/sh -c; command was:\n{command}"
            );
        }
    }
}

#[test]
fn workflow_publish_outside_in_fix_loop_guards_its_inline_git_add_all() {
    let recipe = load_recipe("workflow-publish.yaml");
    let prompt = step_text(&recipe, "step-16b-outside-in-fix-loop", "prompt");
    let guard = prompt
        .find("amplihack hygiene artifact-guard")
        .expect("outside-in fix loop must guard its inline git add -A");
    let add = prompt
        .find("git add -A")
        .expect("outside-in fix loop currently contains broad staging");
    let guarded_line = prompt[guard..]
        .lines()
        .next()
        .expect("guard snippet must be line-oriented");

    assert!(
        guard < add,
        "outside-in fix loop must invoke artifact guard immediately before broad staging"
    );
    assert!(
        guarded_line.contains("&& git add -A"),
        "outside-in fix loop snippet must hard-chain guard success to broad staging; line was `{guarded_line}`"
    );
}

// ---------------------------------------------------------------------------
// Issue #1528: the guard runs again after broad staging; nothing force-adds;
// finalize never commits outside the task worktree.
//
// A guard before `git add -A` cannot see a gitlink that the add itself creates:
// before the add, a registered worktree at a path that is not ignored (such as
// `wt/review`) is just an untracked directory that matches no rule. So every
// commit block runs guard -> add -> guard -> commit.
// ---------------------------------------------------------------------------

const GUARD: &str = "amplihack hygiene artifact-guard";
const SCOPED_TDD_ADD: &str = "git --literal-pathspecs add -A --pathspec-from-file";

/// Byte offset of a shell command at the start of an executable line.
fn shell_command_offset(text: &str, command: &str) -> Option<usize> {
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let indent = line.len() - line.trim_start().len();
        if line[indent..].starts_with(command) {
            return Some(line_start + indent);
        }
        line_start += line.len();
    }
    None
}

/// End of executable shell text on one line; a `#` inside quotes is data.
fn shell_code_end(line: &str) -> usize {
    let (mut single, mut double, mut escaped) = (false, false, false);
    for (at, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if !single => escaped = true,
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '#' if !single
                && !double
                && line[..at]
                    .chars()
                    .last()
                    .is_none_or(|prev| prev.is_whitespace() || matches!(prev, ';' | '&' | '|')) =>
            {
                return at;
            }
            _ => {}
        }
    }
    line.len()
}

/// `(start, end)` byte spans of every broad add (`git add -A`, `git add --all`,
/// `git add .`) in `text`. `git add .gitignore` and `git add -Ab` are not broad
/// adds of the whole tree, so a needle must end at a word boundary. Shell
/// comments do not count as staging commands.
fn broad_add_spans(text: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let code = &line[..shell_code_end(line)];
        for needle in ["git add -A", "git add --all", "git add ."] {
            for (at, _) in code.match_indices(needle) {
                let end = at + needle.len();
                let boundary = code[end..].chars().next().is_none_or(|c| {
                    c.is_whitespace() || matches!(c, ';' | '&' | '|' | ')' | '`' | '"' | '\'')
                });
                if boundary {
                    spans.push((line_start + at, line_start + end));
                }
            }
        }
        line_start += line.len();
    }
    spans.sort_unstable();
    spans
}

/// Offsets of broad adds that are followed by a `git commit` with no guard call
/// between the two.
fn adds_committed_without_recheck(text: &str) -> Vec<usize> {
    broad_add_spans(text)
        .into_iter()
        .filter_map(|(start, end)| {
            let commit = text[end..].find("git commit")?;
            (!text[end..end + commit].contains(GUARD)).then_some(start)
        })
        .collect()
}

/// Every `command` and `prompt` body in a recipe, labelled `<step>.<field>`.
fn step_bodies(recipe: &Value) -> Vec<(String, String)> {
    let mut bodies = Vec::new();
    for step in steps(recipe) {
        let id = step
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        for field in ["command", "prompt"] {
            if let Some(body) = step.get(field).and_then(Value::as_str) {
                bodies.push((format!("{id}.{field}"), body.to_string()));
            }
        }
    }
    bodies
}

#[test]
fn broad_add_recheck_scanner_flags_a_synthetic_unguarded_commit() {
    // Self-check: the scanner below must be able to fail.
    let unguarded = format!("{GUARD} --repo . --mode pre-commit\ngit add -A\ngit commit -m x\n");
    assert_eq!(adds_committed_without_recheck(&unguarded).len(), 1);

    let rechecked = format!(
        "{GUARD} --repo . --mode pre-commit\ngit add -A\n{GUARD} --repo . --mode pre-commit\ngit commit -m x\n"
    );
    assert!(adds_committed_without_recheck(&rechecked).is_empty());

    let one_line = format!(
        "{GUARD} --repo . --mode pre-publish && git add -A && {GUARD} --repo . --mode pre-publish || {{ exit 1; }}\ngit commit -m x\n"
    );
    assert!(adds_committed_without_recheck(&one_line).is_empty());

    for unguarded in [
        "git add --all\nPRE_COMMIT_ALLOW_NO_CONFIG=1 git commit \"$@\"\n",
        "git add . && git commit -m x\n",
    ] {
        assert_eq!(
            adds_committed_without_recheck(unguarded).len(),
            1,
            "must flag `{unguarded}`"
        );
    }

    assert!(
        broad_add_spans("git add .gitignore && git commit -m x").is_empty(),
        "`git add .gitignore` is not a broad add"
    );
    assert_eq!(
        broad_add_spans("# git add -A\necho ready # git add --all\ngit add -A\n").len(),
        1,
        "shell comments must not count as broad staging"
    );
    assert_eq!(
        broad_add_spans("echo '# in quotes'; git add -A\n").len(),
        1,
        "a quoted hash must not hide a later staging command"
    );
    assert!(
        adds_committed_without_recheck("git add -A\necho 'no commit follows'\n").is_empty(),
        "an add with no following commit is out of scope for this rule"
    );
}

#[test]
fn every_recipe_broad_add_followed_by_commit_is_rechecked_by_the_guard() {
    // D5 / R7: guard -> git add -A -> guard -> git commit, in every recipe.
    let mut missing = Vec::new();
    let mut checked = 0;
    for path in recipe_files() {
        let recipe = serde_yaml::from_str::<Value>(&read(&path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        for (label, body) in step_bodies(&recipe) {
            if broad_add_spans(&body).is_empty() {
                continue;
            }
            checked += 1;
            for offset in adds_committed_without_recheck(&body) {
                let line = body[offset..].lines().next().unwrap_or_default();
                missing.push(format!(
                    "{}:{label}: `{}` reaches `git commit` with no guard after staging",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(&path)
                        .display(),
                    line.trim()
                ));
            }
        }
    }

    // The three phase-1 commit sites no longer use a broad add: the TDD
    // checkpoint stages captured paths (#1672) and the refactor and publish
    // steps use `stage_workflow_changes` (#1671). Both are checked in
    // `phase1_commit_sites_never_fall_back_to_repo_path`.
    assert!(
        checked >= 7,
        "expected the seven executable broad-add commit sites to be scanned, found {checked}"
    );
    assert!(
        missing.is_empty(),
        "every broad add that is committed must be re-checked by the artifact guard \
         after staging (a pre-add guard cannot see gitlinks the add creates, #1528):\n{}",
        missing.join("\n")
    );
}

/// The `--mode` value of the guard call starting at byte `at` of `text`.
fn guard_mode_at(text: &str, at: usize) -> Option<&str> {
    let line = text[at..].lines().next()?;
    line.split_once("--mode ")?.1.split_whitespace().next()
}

#[test]
fn post_add_guard_uses_the_same_mode_as_the_pre_add_guard() {
    // Both runs are the same gate: the second must not quietly weaken it.
    let mut mismatched = Vec::new();
    for path in recipe_files() {
        let recipe = serde_yaml::from_str::<Value>(&read(&path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        for (label, body) in step_bodies(&recipe) {
            for (start, end) in broad_add_spans(&body) {
                let Some(commit) = body[end..].find("git commit") else {
                    continue;
                };
                let before = body[..start]
                    .rfind(GUARD)
                    .and_then(|at| guard_mode_at(&body, at));
                let after = body[end..end + commit]
                    .find(GUARD)
                    .and_then(|at| guard_mode_at(&body, end + at));
                if let (Some(before), Some(after)) = (before, after)
                    && before != after
                {
                    mismatched.push(format!(
                        "{}:{label}: pre-add --mode {before} vs post-add --mode {after}",
                        path.display()
                    ));
                }
            }
        }
    }
    assert!(mismatched.is_empty(), "{}", mismatched.join("\n"));
}

#[test]
fn workflow_publish_fix_loop_stops_when_the_post_add_guard_refuses() {
    // The step-16b prompt block runs in a shell without `set -e`. A failed
    // guard must end the shell, or `git commit` on the next line still runs.
    let recipe = load_recipe("workflow-publish.yaml");
    let prompt = step_text(&recipe, "step-16b-outside-in-fix-loop", "prompt");
    let line = prompt
        .lines()
        .find(|line| line.contains("git add -A"))
        .expect("step-16b stages with git add -A");
    let add = line.find("git add -A").expect("add on the line");
    assert!(
        line[..add].contains(&format!("{GUARD} --repo . --mode pre-publish &&")),
        "the first guard is hard-chained to the add: `{line}`"
    );
    assert!(
        line[add..].contains(&format!("&& {GUARD} --repo . --mode pre-publish")),
        "the second guard is chained after the add on the same line: `{line}`"
    );
    let tail = line.rsplit_once("||").map(|(_, tail)| tail).unwrap_or("");
    assert!(
        tail.contains("STOP") && tail.contains("exit 1"),
        "a failure anywhere in the chain must end the shell before git commit: `{line}`"
    );
}

/// Whether `line` runs `git add`/`git stage` with a force option in any
/// spelling: `--force`, or a short cluster containing `f` (`-f`, `-Af`, `-fA`),
/// including `git -C <dir> add` and `git -c <key>=<value> add`.
fn force_adds(line: &str) -> bool {
    let tokens: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || matches!(c, '"' | '\'' | '`' | '(' | ')'))
        .filter(|token| !token.is_empty())
        .collect();
    for (index, token) in tokens.iter().enumerate() {
        if *token != "git" {
            continue;
        }
        let mut sub = index + 1;
        while sub < tokens.len() && tokens[sub].starts_with('-') {
            sub += if matches!(tokens[sub], "-C" | "-c") {
                2
            } else {
                1
            };
        }
        if !matches!(tokens.get(sub), Some(&"add") | Some(&"stage")) {
            continue;
        }
        for arg in &tokens[sub + 1..] {
            if matches!(*arg, "&&" | "||" | ";" | "|" | "--") || arg.ends_with(';') {
                break;
            }
            if *arg == "--force" || arg.starts_with("--force=") {
                return true;
            }
            let short = arg.strip_prefix('-').filter(|rest| !rest.starts_with('-'));
            if short.is_some_and(|rest| {
                !rest.is_empty()
                    && rest.chars().all(|c| c.is_ascii_alphabetic())
                    && rest.contains('f')
            }) {
                return true;
            }
        }
    }
    false
}

fn files_under(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display())) {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            files_under(&path, out);
        } else if path.is_file() {
            out.push(path);
        }
    }
}

#[test]
fn force_add_scanner_recognises_every_spelling() {
    for positive in [
        "git add -f worktrees/feat/x",
        "git add --force -- dist/plugin.js",
        "git -C \"$WT\" add -Af",
        "git -c core.autocrlf=false add -fA .",
        "git stage -f node_modules",
        "cd x && git add -f y",
    ] {
        assert!(force_adds(positive), "must flag `{positive}`");
    }
    for negative in [
        "git add -A",
        "git add --all",
        "git rm --cached -f -- 'worktrees/feat/x'",
        "git add .gitignore",
        "never force a path git refused (#1528)",
        "git add -A && git commit -m fix -f",
        "git commit -m 'git add, then -f'",
    ] {
        assert!(!force_adds(negative), "must not flag `{negative}`");
    }
}

#[test]
fn bundle_recipes_and_tools_never_force_add() {
    // AC8: a forced add is how an ignored run worktree becomes a gitlink.
    let bundle = workspace_root().join("amplifier-bundle");
    let mut files = recipe_files();
    files_under(&bundle.join("tools"), &mut files);

    let mut hits = Vec::new();
    for path in &files {
        let Ok(bytes) = fs::read(path) else { continue };
        let text = String::from_utf8_lossy(&bytes);
        for (number, line) in text.lines().enumerate() {
            if force_adds(line) {
                hits.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(workspace_root())
                        .unwrap_or(path)
                        .display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        hits.is_empty(),
        "no bundled recipe or tool may force-add (#1528):\n{}",
        hits.join("\n")
    );
}

#[test]
fn finalize_push_cleanup_never_commits_in_repo_path_or_cwd() {
    // AC9 / D8 / SEC-11 (static half).
    let recipe = load_recipe("workflow-finalize.yaml");
    let command = step_text(&recipe, "step-20b-push-cleanup", "command");

    for forbidden in [
        "cd \"$REPO_PATH\"",
        "cd $REPO_PATH",
        "falling back to REPO_PATH",
        "using cwd",
    ] {
        assert!(
            !command.contains(forbidden),
            "step-20b must never commit in REPO_PATH or the cwd; found `{forbidden}`"
        );
    }
    let warning = command
        .find("never commits in REPO_PATH (#1528)")
        .expect("step-20b must warn when it skips the cleanup commit");
    let skip = command[warning..]
        .find("exit 0")
        .map(|offset| warning + offset)
        .expect("step-20b must exit 0 (resilient, #414) when it skips");
    let add = command.find("git add -A").expect("step-20b stages");
    assert!(skip < add, "the skip must happen before anything is staged");
    for linked_check in ["--absolute-git-dir", "--git-common-dir"] {
        let at = command
            .find(linked_check)
            .unwrap_or_else(|| panic!("step-20b must verify a linked worktree ({linked_check})"));
        assert!(at < add, "{linked_check} check must precede staging");
    }
}

#[cfg(unix)]
mod finalize_runtime {
    //! AC9 / SEC-11 (runtime half): run step-20b with no usable task worktree
    //! and prove it stages and commits nowhere.

    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    fn hgit(dir: &Path, args: &[&str]) -> String {
        let output = amplihack_git::command()
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn checkout_with_pending_change(dir: &Path) {
        fs::create_dir_all(dir).expect("mkdir");
        hgit(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("README.md"), "base\n").expect("write");
        hgit(dir, &["add", "README.md"]);
        hgit(dir, &["commit", "-qm", "base"]);
        fs::write(dir.join("pending.txt"), "must never be staged here\n").expect("write");
    }

    fn state(dir: &Path) -> (String, String, String) {
        (
            hgit(dir, &["rev-parse", "HEAD"]),
            hgit(dir, &["diff", "--cached", "--raw"]),
            hgit(dir, &["ls-files", "--others", "--exclude-standard"]),
        )
    }

    #[test]
    fn finalize_push_cleanup_skips_without_a_linked_task_worktree() {
        let command = step_text(
            &load_recipe("workflow-finalize.yaml"),
            "step-20b-push-cleanup",
            "command",
        )
        .to_string();
        let tmp = TempDir::new().expect("tempdir");
        let launch = tmp.path().join("launch");
        let cwd = tmp.path().join("cwd");
        checkout_with_pending_change(&launch);
        checkout_with_pending_change(&cwd);

        let bin = tmp.path().join("bin");
        fs::create_dir_all(&bin).expect("mkdir bin");
        let stub = bin.join("amplihack");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                tmp.path().join("amplihack.log").display()
            ),
        )
        .expect("write stub");
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");

        let missing = tmp.path().join("gone");
        let cases: [(&str, Option<&Path>); 3] = [
            ("unset", None),
            ("missing directory", Some(&missing)),
            ("main checkout, not a linked worktree", Some(&launch)),
        ];
        for (label, worktree) in cases {
            let before = (state(&launch), state(&cwd));
            let mut run = Command::new("bash");
            run.arg("-c")
                .arg(&command)
                .current_dir(&cwd)
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", tmp.path())
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("REPO_PATH", &launch)
                .env("AMPLIHACK_HOME", workspace_root());
            if let Some(worktree) = worktree {
                run.env("WORKTREE_SETUP_WORKTREE_PATH", worktree);
            }
            let output = run.output().expect("run step-20b");
            let stderr = String::from_utf8_lossy(&output.stderr);

            assert_eq!(
                output.status.code(),
                Some(0),
                "{label}: step-20b stays resilient (#414)\nstderr:\n{stderr}"
            );
            assert!(
                stderr.contains("WARNING") && stderr.contains("#1528"),
                "{label}: must say why it skipped\nstderr:\n{stderr}"
            );
            assert_eq!(
                (state(&launch), state(&cwd)),
                before,
                "{label}: nothing may be staged or committed in REPO_PATH or the cwd"
            );
            assert!(
                !tmp.path().join("amplihack.log").exists(),
                "{label}: the skip happens before the guard or any staging"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Issue #1528: every guard call can actually stop its step.
//
// Bash steps run under `set -e`, which ignores a failure inside an `if`
// condition, inside `$(…)`, after `!`, and on the left of `&&`/`||`. Agent
// prompt blocks run without `set -e`, so a guard there must end the shell
// itself. A guard that refuses and lets the step reach `git commit` is no gate.
// ---------------------------------------------------------------------------

/// Comment-free logical lines, `\` continuations joined.
fn shell_logical_lines(body: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for raw in body.lines() {
        let line = raw.trim();
        if current.is_empty() && line.starts_with('#') {
            continue;
        }
        if let Some(stripped) = line.strip_suffix('\\') {
            current.push_str(stripped);
            current.push(' ');
            continue;
        }
        current.push_str(line);
        lines.push(std::mem::take(&mut current));
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The lines inside ```bash fences of an agent prompt, joined as above.
fn fenced_shell_lines(prompt: &str) -> Vec<String> {
    let mut body = String::new();
    let mut fenced = false;
    for line in prompt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            fenced = !fenced && trimmed.starts_with("```bash");
            continue;
        }
        if fenced {
            body.push_str(line);
            body.push('\n');
        }
    }
    shell_logical_lines(&body)
}

/// Whether `line` ends with an explicit `|| { …; exit 1; }` group.
fn ends_with_explicit_stop(line: &str) -> bool {
    let trimmed = line.trim_end();
    let Some((_, tail)) = trimmed.rsplit_once("||") else {
        return false;
    };
    let tail = tail.trim();
    tail.starts_with('{') && tail.ends_with('}') && tail.contains("exit 1")
}

/// Why the guard call on `line` cannot stop its step, if it cannot.
/// `set_e`: the body runs under `set -e`.
fn non_binding_guard(line: &str, set_e: bool) -> Option<&'static str> {
    let at = line.find(GUARD)?;
    let prefix = &line[..at];
    if prefix
        .rfind("$(")
        .is_some_and(|open| !prefix[open..].contains(')'))
        || prefix.matches('`').count() % 2 == 1
    {
        return Some("inside a command substitution");
    }
    let segment = [";", "&&", "||", "{", " then ", " do "]
        .iter()
        .filter_map(|sep| prefix.rfind(sep).map(|i| i + sep.len()))
        .max()
        .map_or(prefix, |start| &prefix[start..])
        .trim();
    for word in ["if", "elif", "while", "until", "!"] {
        if segment == word || segment.starts_with(&format!("{word} ")) {
            return Some("inside a condition or negated");
        }
    }
    let explicit = ends_with_explicit_stop(line);
    if (line.contains("&&") || line.contains("||")) && !explicit {
        return Some("in an &&/|| list without a final `|| { …; exit 1; }`");
    }
    if !set_e && !explicit {
        return Some("no `set -e` and no `|| { …; exit 1; }`");
    }
    None
}

#[test]
fn binding_guard_scanner_flags_every_non_binding_form() {
    // Self-check: the scanner below must be able to fail.
    let g = format!("{GUARD} --repo . --mode pre-publish");
    let stop = r#"|| { echo "STOP: artifact guard refused" >&2; exit 1; }"#;
    for binding in [
        g.clone(),
        format!("git add -A; {g}"),
        format!("{g} {stop}"),
        format!("{g} && git add -A && {g} {stop}"),
    ] {
        assert_eq!(non_binding_guard(&binding, true), None, "`{binding}`");
    }
    for unbound in [
        format!("git add -A && {g}"),
        format!("{g} && git add -A"),
        format!("{g} || true"),
        format!("{g} || echo 'guard refused'"),
        format!("if {g}; then git commit -m x; fi"),
        format!("if ! {g}; then exit 1; fi"),
        format!("! {g}"),
        format!("OUT=\"$({g})\""),
        format!("OUT=`{g}`"),
    ] {
        assert!(
            non_binding_guard(&unbound, true).is_some(),
            "must flag `{unbound}`"
        );
    }
    assert!(
        non_binding_guard(&g, false).is_some(),
        "a bare guard in a prompt"
    );
    assert_eq!(non_binding_guard(&format!("{g} {stop}"), false), None);
}

#[test]
fn every_guard_call_in_every_recipe_can_stop_its_step() {
    let mut problems = Vec::new();
    let mut checked = 0;
    for path in recipe_files() {
        let recipe = serde_yaml::from_str::<Value>(&read(&path))
            .unwrap_or_else(|e| panic!("parse {}: {e}", path.display()));
        let name = path
            .strip_prefix(workspace_root())
            .unwrap_or(&path)
            .display()
            .to_string();
        for step in steps(&recipe) {
            let id = step
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let mut bodies = Vec::new();
            if let Some(command) = step.get("command").and_then(Value::as_str) {
                let set_e = command.contains("set -e");
                bodies.push((shell_logical_lines(command), set_e, "command"));
            }
            if let Some(prompt) = step.get("prompt").and_then(Value::as_str) {
                bodies.push((fenced_shell_lines(prompt), false, "prompt"));
            }
            for (lines, set_e, field) in bodies {
                for line in lines.iter().filter(|line| line.contains(GUARD)) {
                    checked += 1;
                    if let Some(why) = non_binding_guard(line, set_e) {
                        problems.push(format!("{name}:{id}.{field}: {why}: `{line}`"));
                    }
                }
            }
        }
    }
    assert!(
        checked >= 20,
        "expected the pre- and post-add guards at every site, found {checked}"
    );
    assert!(
        problems.is_empty(),
        "a guard that refuses must stop the step before anything is committed (#1528):\n{}",
        problems.join("\n")
    );
}

#[test]
fn checkpoint_and_review_recheck_after_staging() {
    let tdd = load_recipe("workflow-tdd.yaml");
    let command = step_text(&tdd, "checkpoint-after-implementation", "command");
    let scoped_add = shell_command_offset(command, SCOPED_TDD_ADD)
        .expect("TDD checkpoint must stage only captured paths");
    let commit = command
        .find("commit_with_pre_commit_guard -m")
        .expect("TDD checkpoint must invoke the guarded commit helper");
    assert!(
        scoped_add < commit
            && command[scoped_add + SCOPED_TDD_ADD.len()..commit]
                .lines()
                .any(|line| line.trim()
                    == "amplihack hygiene artifact-guard --repo . --mode pre-publish"),
        "TDD checkpoint must recheck artifacts between scoped staging and commit: {command}"
    );

    // Review feedback still uses broad staging and keeps the guard on the
    // same line, so `set -e` binds a failed add before the guard can run.
    let review = load_recipe("workflow-pr-review.yaml");
    let command = step_text(&review, "step-18c-push-feedback-changes", "command");
    assert!(
        command.lines().any(|line| line.trim()
            == "git add -A; amplihack hygiene artifact-guard --repo . --mode pre-publish"),
        "workflow-pr-review.yaml must recheck artifacts after broad staging: {command}"
    );
}

#[test]
fn recipes_touched_by_issue_1528_stay_under_the_brick_budget() {
    // AC10. The phase recipes are covered by
    // `every_phase_subrecipe_under_400_lines`; these are not. The budget is
    // the shared `BRICK_LINE_BUDGET` (at most 400 lines), not a second copy.
    use crate::auto_drive_to_merge::BRICK_LINE_BUDGET;
    for recipe in [
        "autodrive-crusty-loop.yaml",
        "autodrive-merge-loop.yaml",
        "autodrive-crusty-round.yaml",
        "autodrive-merge-round.yaml",
        "consensus-publish.yaml",
        "consensus-pr-feedback.yaml",
    ] {
        let lines = read(&recipe_path(recipe)).lines().count();
        assert!(
            lines <= BRICK_LINE_BUDGET,
            "{recipe} has {lines} lines; the brick budget is {BRICK_LINE_BUDGET}"
        );
    }
}

const STEP_20B_CONTEXT_DEFAULT: &str =
    r#": "${WORKTREE_SETUP_WORKTREE_PATH:=${RECIPE_VAR_worktree_setup__worktree_path:-}}""#;

fn step_20b_warning(reason: &str) -> String {
    format!(
        "WARNING: step-20b {reason}; skipping cleanup commit/push — never commits in REPO_PATH (#1528)"
    )
}

#[test]
fn finalize_push_cleanup_skips_before_the_runtime_preflight() {
    // D2: the preflight can `rm -rf` a main checkout's `worktrees/`, so the
    // location check runs first. The context default line is untouched (AC12,
    // owned by fix/autodrive-step-output-plumbing).
    let recipe = load_recipe("workflow-finalize.yaml");
    let command = step_text(&recipe, "step-20b-push-cleanup", "command");
    assert!(
        command
            .lines()
            .any(|line| line.trim() == STEP_20B_CONTEXT_DEFAULT),
        "step-20b keeps `{STEP_20B_CONTEXT_DEFAULT}` byte-identical:\n{command}"
    );
    for reason in [
        "worktree path gone",
        "worktree path is not a linked worktree",
    ] {
        assert!(
            command.contains(reason),
            "step-20b must name the skip reason `{reason}`"
        );
    }
    assert!(
        command.contains("--path-format=absolute --git-common-dir"),
        "the common dir is read as an absolute path"
    );
    let last_skip = command
        .rfind("never commits in REPO_PATH (#1528)")
        .expect("skip warning");
    for later in [
        "preflight_known_workflow_runtime_artifacts",
        "cleanup_known_workflow_runtime_artifacts",
        GUARD,
        "git add -A",
    ] {
        if let Some(at) = command.find(later) {
            assert!(
                last_skip < at,
                "every skip must come before `{later}` in step-20b"
            );
        }
    }
}

#[cfg(unix)]
mod finalize_skip_reasons {
    //! D2 runtime: each skip names its reason, exits 0, and never reaches the
    //! runtime preflight, which would delete a main checkout's `worktrees/`.

    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    fn hgit(dir: &Path, args: &[&str]) -> String {
        let output = amplihack_git::command()
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    #[test]
    fn finalize_push_cleanup_names_its_reason_and_leaves_a_main_checkout_alone() {
        let command = step_text(
            &load_recipe("workflow-finalize.yaml"),
            "step-20b-push-cleanup",
            "command",
        )
        .to_string();
        let tmp = TempDir::new().expect("tempdir");
        let root = fs::canonicalize(tmp.path()).expect("canonicalize");

        // A main checkout holding another run's live worktree and runtime
        // state, both of which the preflight would delete.
        let launch = root.join("launch");
        fs::create_dir_all(&launch).expect("mkdir");
        hgit(&launch, &["init", "-q", "-b", "main"]);
        fs::write(launch.join("README.md"), "base\n").expect("write");
        fs::write(launch.join(".gitignore"), "worktrees/\n.claude/runtime/\n").expect("write");
        hgit(&launch, &["add", "README.md", ".gitignore"]);
        hgit(&launch, &["commit", "-qm", "base"]);
        hgit(
            &launch,
            &[
                "worktree",
                "add",
                "-q",
                "worktrees/feat/other",
                "-b",
                "other",
            ],
        );
        let other_wip = launch.join("worktrees/feat/other/wip.txt");
        fs::write(&other_wip, "another run's uncommitted work\n").expect("write");
        fs::create_dir_all(launch.join(".claude/runtime")).expect("mkdir");
        fs::write(launch.join(".claude/runtime/state.json"), "{}\n").expect("write");

        let plain = root.join("plain");
        fs::create_dir_all(&plain).expect("mkdir");
        let file = root.join("file");
        fs::write(&file, "not a directory\n").expect("write");

        let bin = root.join("bin");
        fs::create_dir_all(&bin).expect("mkdir bin");
        let stub = bin.join("amplihack");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                root.join("amplihack.log").display()
            ),
        )
        .expect("write stub");
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");

        let gone = "worktree path gone";
        let not_linked = "worktree path is not a linked worktree";
        let not_a_repo = format!(
            "not a git repo at {} (#537)",
            fs::canonicalize(&plain)
                .expect("canonicalize plain")
                .display()
        );
        let cases: [(&str, Option<&Path>, &str); 5] = [
            ("unset", None, gone),
            ("missing", Some(&root.join("missing")), gone),
            ("a file", Some(&file), gone),
            ("not a git directory", Some(&plain), &not_a_repo),
            ("main checkout", Some(&launch), not_linked),
        ];
        for (label, worktree, reason) in cases {
            let head = hgit(&launch, &["rev-parse", "HEAD"]);
            let mut run = Command::new("bash");
            run.arg("-c")
                .arg(&command)
                .current_dir(&launch)
                .env_clear()
                .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
                .env("HOME", root.join("home"))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_TERMINAL_PROMPT", "0")
                .env("GIT_CEILING_DIRECTORIES", &root)
                .env("REPO_PATH", &launch)
                .env("AMPLIHACK_HOME", workspace_root());
            if let Some(worktree) = worktree {
                run.env("WORKTREE_SETUP_WORKTREE_PATH", worktree);
            }
            let output = run.output().expect("run step-20b");
            let stderr = String::from_utf8_lossy(&output.stderr);

            assert_eq!(output.status.code(), Some(0), "{label}\nstderr:\n{stderr}");
            assert!(
                stderr.lines().any(|line| line == step_20b_warning(reason)),
                "{label}: expected `{}`\nstderr:\n{stderr}",
                step_20b_warning(reason)
            );
            assert!(
                other_wip.exists() && launch.join(".claude/runtime/state.json").exists(),
                "{label}: the runtime preflight must not run in the launch checkout"
            );
            assert_eq!(hgit(&launch, &["rev-parse", "HEAD"]), head, "{label}");
            assert!(
                hgit(&launch, &["diff", "--cached", "--raw"]).is_empty(),
                "{label}: nothing staged in the launch checkout"
            );
            assert!(
                !root.join("amplihack.log").exists(),
                "{label}: the skip happens before the guard"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Issue #1528: the phase-1 commit sites (tdd checkpoint, refactor-review
// checkpoint, publish step-15) commit only at the top of the linked task
// worktree. Unlike 20a/20b they hard-fail rather than skip: a skipped
// checkpoint or publish commit would lose work silently.
// ---------------------------------------------------------------------------

const PHASE1_COMMIT_SITES: [(&str, &str); 3] = [
    ("workflow-tdd.yaml", "checkpoint-after-implementation"),
    (
        "workflow-refactor-review.yaml",
        "checkpoint-after-review-feedback",
    ),
    ("workflow-publish.yaml", "step-15-commit-push"),
];

const PHASE1_REFUSAL_TAIL: &str = " is not the top of a linked task worktree; refusing to stage or commit; never commits in REPO_PATH (#1528)";

#[test]
fn phase1_commit_sites_never_fall_back_to_repo_path() {
    for (recipe, step) in PHASE1_COMMIT_SITES {
        let loaded = load_recipe(recipe);
        let command = step_text(&loaded, step, "command");
        let add_command = if step == "checkpoint-after-implementation" {
            SCOPED_TDD_ADD
        } else {
            "stage_workflow_changes; amplihack hygiene artifact-guard"
        };
        let add = shell_command_offset(command, add_command)
            .unwrap_or_else(|| panic!("{recipe}/{step} must execute `{add_command}`"));
        assert!(
            command.contains(STEP_20B_CONTEXT_DEFAULT),
            "{recipe}/{step}: the worktree default must not fall back to REPO_PATH"
        );
        assert!(
            !command[..add].contains("${REPO_PATH:-}}}"),
            "{recipe}/{step}: no `${{REPO_PATH:-}}` fallback before staging"
        );
        for linked_check in ["--absolute-git-dir", "--git-common-dir", "pwd -P"] {
            let at = command.find(linked_check).unwrap_or_else(|| {
                panic!("{recipe}/{step} must verify a linked worktree ({linked_check})")
            });
            assert!(
                at < add,
                "{recipe}/{step}: {linked_check} must precede staging"
            );
        }
        let refusal = command
            .find("never commits in REPO_PATH (#1528)")
            .unwrap_or_else(|| panic!("{recipe}/{step} must name #1528 when it refuses"));
        let exit = command[refusal..]
            .find("exit ")
            .map(|offset| refusal + offset)
            .unwrap_or_else(|| panic!("{recipe}/{step}: the refusal must exit"));
        assert!(
            command[exit..].starts_with("exit 1;") && exit < add,
            "{recipe}/{step}: the refusal must hard-fail with exit 1 before staging, not skip"
        );
    }
}

#[cfg(unix)]
mod phase1_commit_runtime {
    //! Run each phase-1 commit site with no usable task worktree and prove it
    //! fails, stages nothing and commits nowhere; then run it at the top of a
    //! linked worktree and prove the check lets it through.

    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use tempfile::TempDir;

    fn hgit(dir: &Path, args: &[&str]) -> String {
        let output = amplihack_git::command()
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.invalid")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.invalid")
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn checkout_with_pending_change(dir: &Path) {
        fs::create_dir_all(dir).expect("mkdir");
        hgit(dir, &["init", "-q", "-b", "main"]);
        fs::write(dir.join("README.md"), "base\n").expect("write");
        hgit(dir, &["add", "README.md"]);
        hgit(dir, &["commit", "-qm", "base"]);
        fs::write(dir.join("pending.txt"), "must never be staged here\n").expect("write");
    }

    fn state(dir: &Path) -> (String, String, String) {
        (
            hgit(dir, &["rev-parse", "HEAD"]),
            hgit(dir, &["diff", "--cached", "--raw"]),
            hgit(dir, &["ls-files", "--others", "--exclude-standard"]),
        )
    }

    fn run(
        command: &str,
        root: &Path,
        cwd: &Path,
        launch: &Path,
        worktree: Option<&Path>,
    ) -> std::process::Output {
        let mut run = Command::new("bash");
        run.arg("-c")
            .arg(command)
            .current_dir(cwd)
            .env_clear()
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", root.join("bin").display()),
            )
            .env("HOME", root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("REPO_PATH", launch)
            .env("AMPLIHACK_HOME", workspace_root());
        if let Some(worktree) = worktree {
            run.env("WORKTREE_SETUP_WORKTREE_PATH", worktree);
        }
        run.output().expect("run step")
    }

    #[test]
    fn phase1_commit_sites_hard_fail_outside_a_linked_task_worktree() {
        for (recipe, step) in PHASE1_COMMIT_SITES {
            let command = step_text(&load_recipe(recipe), step, "command").to_string();
            let tmp = TempDir::new().expect("tempdir");
            let root = fs::canonicalize(tmp.path()).expect("canonicalize");
            let launch = root.join("launch");
            let cwd = root.join("cwd");
            checkout_with_pending_change(&launch);
            checkout_with_pending_change(&cwd);
            let linked = root.join("linked");
            hgit(
                &launch,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    "task",
                    linked.to_str().unwrap(),
                ],
            );
            fs::create_dir_all(linked.join("sub")).expect("mkdir sub");
            fs::write(linked.join("sub/pending.txt"), "not from the top\n").expect("write");

            let bin = root.join("bin");
            fs::create_dir_all(&bin).expect("mkdir bin");
            let log = root.join("amplihack.log");
            let stub = bin.join("amplihack");
            fs::write(
                &stub,
                format!(
                    "#!/usr/bin/env bash\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
                    log.display()
                ),
            )
            .expect("write stub");
            fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).expect("chmod");

            let missing = root.join("gone");
            let subdir = linked.join("sub");
            let cases: [(&str, Option<&Path>, bool); 4] = [
                ("unset", None, false),
                ("missing directory", Some(&missing), false),
                ("main checkout, not a linked worktree", Some(&launch), true),
                ("subdirectory of a linked worktree", Some(&subdir), true),
            ];
            for (label, worktree, names_1528) in cases {
                let before = (state(&launch), state(&cwd), state(&linked));
                let output = run(&command, &root, &cwd, &launch, worktree);
                let stderr = String::from_utf8_lossy(&output.stderr);

                assert_ne!(
                    output.status.code(),
                    Some(0),
                    "{step}/{label}: must hard-fail, not skip\nstderr:\n{stderr}"
                );
                if names_1528 {
                    assert!(
                        stderr
                            .lines()
                            .any(|line| line.starts_with(&format!("ERROR: {step}: "))
                                && line.ends_with(PHASE1_REFUSAL_TAIL)),
                        "{step}/{label}: expected the #1528 refusal\nstderr:\n{stderr}"
                    );
                }
                assert_eq!(
                    (state(&launch), state(&cwd), state(&linked)),
                    before,
                    "{step}/{label}: nothing may be staged or committed anywhere"
                );
                assert!(
                    !log.exists(),
                    "{step}/{label}: the refusal happens before the guard or any staging"
                );
            }

            // At the top of the linked worktree the check lets the step through
            // to the artifact guard, and the launch checkout stays untouched.
            let launch_before = state(&launch);
            let output = run(&command, &root, &cwd, &launch, Some(&linked));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !stderr.contains("not the top of a linked task worktree"),
                "{step}: a linked worktree top must pass the check\nstderr:\n{stderr}"
            );
            if step != "step-15-commit-push" || stderr.contains("Staging Changes") {
                assert!(
                    fs::read_to_string(&log).is_ok_and(|l| l.contains("artifact-guard")),
                    "{step}: the step must reach the artifact guard in the linked worktree\nstderr:\n{stderr}"
                );
            }
            assert_eq!(
                state(&launch),
                launch_before,
                "{step}: launch checkout untouched"
            );
        }
    }
}
