//! Issue #1619 (TDD): overriding one step's tier.
//!
//! Staging has no output channel of its own: it hands its diagnostics back
//! and `execute.rs` renders them, once per run. A step id nothing matches is
//! the likeliest operator error in this feature and the one with no other
//! symptom, so it is a notice rather than a silence.
//!
//! Why the override is applied recipe-side rather than at launch: the step id
//! never reaches the child process. The recipe runner sets no step-id
//! environment variable, so amplihack cannot know at launch time which step a
//! command belongs to. Patching the recipe is the honest way to change one
//! step's tier, and the patch is line-scoped and vocabulary-closed — only one
//! of three literal words is ever written into a recipe file.
//!
//! Homes and recipe directories here are tempfile directories only.

use super::step_tiers::{Outcome, RECIPE_DIR_ENV, Staged, stage, validate_step_id};
use crate::commands::launch::model_tiers::notice::Notice;
use crate::commands::launch::model_tiers::table::Tier;
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

const RECIPE: &str = r#"name: "sample"
steps:
  - id: "step-08-implement"
    agent: "amplihack:builder"
    model: "strong"
    prompt: |
      Build it.
  - id: "step-20-final-cleanup"
    agent: "amplihack:cleanup"
    prompt: |
      Tidy up.
  - id: "run-the-build"
    type: "bash"
    command: |
      echo building
"#;

struct Fixture {
    _tmp: tempfile::TempDir,
    run_dir: PathBuf,
    recipes: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        let run_dir = root.join("run");
        let recipes = root.join("recipes");
        fs::create_dir_all(&run_dir).unwrap();
        fs::create_dir_all(&recipes).unwrap();
        fs::write(recipes.join("sample.yaml"), RECIPE).unwrap();
        Self {
            _tmp: tmp,
            run_dir,
            recipes,
        }
    }

    fn recipe(&self) -> PathBuf {
        self.recipes.join("sample.yaml")
    }

    fn stage(&self, overrides: &[(&str, Tier)]) -> Result<Outcome, String> {
        let map: BTreeMap<String, Tier> = overrides
            .iter()
            .map(|(id, tier)| ((*id).to_string(), *tier))
            .collect();
        stage(
            &self.run_dir,
            std::slice::from_ref(&self.recipes),
            &self.recipe(),
            &map,
        )
    }

    /// The staged copies of an override that is expected to match a step.
    fn staged(&self, overrides: &[(&str, Tier)]) -> Staged {
        let outcome = self.stage(overrides).expect("stage");
        assert!(
            outcome.notices.is_empty(),
            "an override that matches a step reports nothing: {:?}",
            outcome.notices
        );
        outcome.staged.expect("one file is staged")
    }
}

fn staged_text(staged: &Staged, name: &str) -> String {
    fs::read_to_string(staged.dir.join(name)).unwrap_or_else(|e| panic!("read staged {name}: {e}"))
}

/// The `model:` a step declares in `text`, read the simple way a reader of
/// the file would.
fn model_of(text: &str, step_id: &str) -> Option<String> {
    let recipe: serde_yaml::Value = serde_yaml::from_str(text).expect("staged YAML parses");
    recipe
        .get("steps")?
        .as_sequence()?
        .iter()
        .find(|s| s.get("id").and_then(serde_yaml::Value::as_str) == Some(step_id))?
        .get("model")?
        .as_str()
        .map(str::to_string)
}

// -------------------------------------------------------------------------
// The no-override case is pinned first
// -------------------------------------------------------------------------

#[test]
fn nothing_is_staged_when_no_step_tier_is_configured() {
    let f = Fixture::new();
    let outcome = f.stage(&[]).expect("no overrides is not an error");
    assert!(
        outcome.notices.is_empty(),
        "nothing configured is nothing to say: {:?}",
        outcome.notices
    );
    assert!(
        outcome.staged.is_none(),
        "with nothing configured, nothing is staged, no environment variable is set, \
         and the run behaves byte for byte as it did before"
    );
    assert!(
        fs::read_dir(&f.run_dir).unwrap().next().is_none(),
        "no directory is created either"
    );
}

#[test]
fn the_environment_variable_is_the_runners_first_search_directory() {
    assert_eq!(
        RECIPE_DIR_ENV, "AMPLIHACK_PACKAGE_RECIPE_DIR",
        "the staged copies must be found before the bundled ones, and amplihack sets \
         this variable nowhere else"
    );
}

// -------------------------------------------------------------------------
// Patching
// -------------------------------------------------------------------------

#[test]
fn an_existing_model_line_is_rewritten() {
    let f = Fixture::new();
    let staged = f.staged(&[("step-08-implement", Tier::Standard)]);
    let text = staged_text(&staged, "sample.yaml");
    assert_eq!(
        model_of(&text, "step-08-implement").as_deref(),
        Some("standard")
    );
}

#[test]
fn a_step_with_no_model_line_gains_one() {
    let f = Fixture::new();
    let staged = f.staged(&[("step-20-final-cleanup", Tier::Fast)]);
    let text = staged_text(&staged, "sample.yaml");
    assert_eq!(
        model_of(&text, "step-20-final-cleanup").as_deref(),
        Some("fast")
    );
}

#[test]
fn only_the_named_step_is_touched() {
    let f = Fixture::new();
    let staged = f.staged(&[("step-20-final-cleanup", Tier::Fast)]);
    let text = staged_text(&staged, "sample.yaml");
    assert_eq!(
        model_of(&text, "step-08-implement").as_deref(),
        Some("strong"),
        "the other step keeps the tier its recipe declares"
    );
    assert!(
        text.contains("echo building"),
        "the bash step body is copied, never regenerated: {text}"
    );
    assert!(
        text.contains("Build it."),
        "prompts are copied verbatim: a patched recipe is the same recipe"
    );
}

#[test]
fn only_one_of_three_literal_words_is_ever_written() {
    // The patch is vocabulary-closed, so no text from an override file can
    // reach a prompt or a shell body.
    let f = Fixture::new();
    for tier in [Tier::Fast, Tier::Standard, Tier::Strong] {
        let staged = f.staged(&[("step-08-implement", tier)]);
        let text = staged_text(&staged, "sample.yaml");
        assert_eq!(
            model_of(&text, "step-08-implement").as_deref(),
            Some(tier.as_str())
        );
    }
}

#[test]
fn a_step_id_no_recipe_contains_is_reported_and_stages_nothing() {
    // The likeliest operator error in this feature, and the one with no
    // other symptom: the run simply proceeds on the tiers the recipes
    // declare. Staging nothing is not a report, so there is a line.
    let f = Fixture::new();
    let outcome = f
        .stage(&[("step-99-polish", Tier::Fast)])
        .expect("an unknown step id is reported, not an error");
    assert_eq!(
        outcome.notices,
        vec![Notice::StepTierUnmatched {
            step: "step-99-polish".to_string()
        }],
        "an id no recipe declares is named back to the operator"
    );
    assert!(
        outcome.staged.is_none(),
        "only files that contain a named step id are staged"
    );
}

#[test]
fn a_step_id_that_matches_is_not_reported_as_unmatched() {
    let f = Fixture::new();
    let outcome = f
        .stage(&[
            ("step-08-implement", Tier::Fast),
            ("step-99-polish", Tier::Fast),
        ])
        .expect("stage");
    assert_eq!(
        outcome.notices,
        vec![Notice::StepTierUnmatched {
            step: "step-99-polish".to_string()
        }],
        "only the id nothing declared is reported: {:?}",
        outcome.notices
    );
    let staged = outcome
        .staged
        .expect("the matching id still stages its recipe");
    assert_eq!(
        model_of(&staged_text(&staged, "sample.yaml"), "step-08-implement").as_deref(),
        Some("fast")
    );
}

#[test]
fn only_files_containing_a_named_step_are_copied() {
    let f = Fixture::new();
    fs::write(
        f.recipes.join("other.yaml"),
        "name: \"other\"\nsteps:\n  - id: \"x\"\n    type: \"bash\"\n    command: \"true\"\n",
    )
    .unwrap();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    assert!(
        !staged.dir.join("other.yaml").exists(),
        "a recipe with no named step is left to be found in its own directory"
    );
}

#[test]
fn a_recipe_that_only_mentions_a_named_step_is_not_copied() {
    // `patch` skips a file that does not contain a wanted id as a substring,
    // because a rewritten id is read out of a line of the file. That is a
    // prefilter and never the decision: a file that names the step in prose
    // but declares no such step still has to come back unstaged, and the line
    // walk is what decides that.
    let f = Fixture::new();
    fs::write(
        f.recipes.join("mentions.yaml"),
        "name: \"mentions\"\nsteps:\n  - id: \"x\"\n    agent: \"a\"\n    prompt: |\n      \
         Read what step-08-implement produced.\n",
    )
    .unwrap();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    assert!(
        !staged.dir.join("mentions.yaml").exists(),
        "naming a step in a prompt does not declare it, so nothing is patched"
    );
}

#[test]
fn a_model_line_inside_a_step_body_is_not_mistaken_for_the_field() {
    // The `model:` the patch rewrites is the step's own field, at the step's
    // field indent. A prompt or a shell line that happens to begin `model:`
    // is the step's content: replacing it would both corrupt the body and
    // leave the step with no tier. No bundled recipe has that shape today,
    // so without this the invariant would live in the data, not the code.
    let f = Fixture::new();
    fs::write(
        f.recipes.join("sample.yaml"),
        "name: \"sample\"\nsteps:\n  - id: \"step-08-implement\"\n    agent: \"a\"\n    \
         prompt: |\n      model: whatever the caller asked for\n      Build it.\n",
    )
    .unwrap();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    let text = staged_text(&staged, "sample.yaml");
    assert!(
        text.contains("model: whatever the caller asked for"),
        "the prompt is copied verbatim:\n{text}"
    );
    assert_eq!(
        model_of(&text, "step-08-implement").as_deref(),
        Some("fast"),
        "and the step still gains the tier it was given:\n{text}"
    );
}

// -------------------------------------------------------------------------
// Step ids are identifiers, never path components
// -------------------------------------------------------------------------

#[test]
fn step_ids_amplihack_accepts() {
    for good in [
        "step-08-implement",
        "step-10b-security-review",
        "a",
        "A1",
        "a.b_c-d",
        &"x".repeat(128),
    ] {
        assert!(validate_step_id(good), "`{good}` must be accepted");
    }
}

#[test]
fn step_ids_amplihack_rejects() {
    for bad in [
        "",
        ".",
        "..",
        "../etc/passwd",
        "a/b",
        "a\\b",
        "-leading-dash",
        ".leading-dot",
        "a b",
        "a\0b",
        "a\nb",
        "a;b",
        "a$b",
        &"x".repeat(129),
    ] {
        assert!(!validate_step_id(bad), "`{bad:?}` must be rejected");
    }
}

#[test]
fn a_rejected_step_id_is_reported_and_stages_nothing() {
    let f = Fixture::new();
    let mut map: BTreeMap<String, Tier> = BTreeMap::new();
    map.insert("../../escape".to_string(), Tier::Fast);
    let outcome = stage(
        &f.run_dir,
        std::slice::from_ref(&f.recipes),
        &f.recipe(),
        &map,
    )
    .expect("a bad step id is reported, not fatal");
    assert_eq!(
        outcome.notices,
        vec![Notice::BadStepId {
            step: "../../escape".to_string()
        }],
        "a rejected id is named back to the operator rather than dropped in silence"
    );
    assert!(
        outcome.staged.is_none(),
        "a step id that is not an identifier never reaches a path join"
    );
}

#[test]
fn a_rejected_step_id_is_escaped_where_it_is_printed() {
    // The id comes from an operator's file and the line lands in a terminal.
    let line = Notice::BadStepId {
        step: "a\u{1b}[31mb".to_string(),
    }
    .render();
    assert!(
        !line.contains('\u{1b}') && line.contains("\\u{1b}"),
        "a control character is escaped, not printed: {line:?}"
    );
}

#[test]
fn staged_file_names_come_only_from_the_discovered_recipe() {
    let f = Fixture::new();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    for file in &staged.files {
        assert_eq!(
            file.parent(),
            Some(staged.dir.as_path()),
            "every staged file is directly inside the staging directory: {file:?}"
        );
        assert_eq!(
            file.file_name(),
            Some(std::ffi::OsStr::new("sample.yaml")),
            "the name comes from the recipe amplihack found, never from the override \
             file"
        );
    }
}

// -------------------------------------------------------------------------
// The staging directory
// -------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn the_staging_directory_and_its_files_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    let dir_mode = fs::metadata(&staged.dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "directory mode was {dir_mode:o}");
    for file in &staged.files {
        let mode = fs::metadata(file).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{file:?} mode was {mode:o}");
    }
}

#[test]
fn the_staging_directory_lives_under_the_run_directory() {
    let f = Fixture::new();
    let staged = f.staged(&[("step-08-implement", Tier::Fast)]);
    assert!(
        staged.dir.starts_with(&f.run_dir),
        "the patched copies belong to the run that made them: {:?}",
        staged.dir
    );
}

#[test]
fn the_original_recipe_file_is_never_modified() {
    let f = Fixture::new();
    let before = fs::read_to_string(f.recipe()).unwrap();
    let _ = f.staged(&[("step-08-implement", Tier::Fast)]);
    assert_eq!(
        fs::read_to_string(f.recipe()).unwrap(),
        before,
        "staging writes copies; the bundled recipe is read-only to this feature"
    );
}

#[test]
fn a_recipe_directory_that_cannot_be_read_is_skipped_rather_than_fatal() {
    // Skipped without a line of its own: the runner would have skipped it
    // too, so a directory amplihack cannot read is not news. The
    // operator-visible consequence — a configured step id that now matches
    // nothing — is what gets reported, and this fixture's top-level recipe
    // still declares the step, so there is nothing to say here at all.
    let f = Fixture::new();
    let missing: PathBuf = f.run_dir.join("no-such-recipe-dir");
    let mut map: BTreeMap<String, Tier> = BTreeMap::new();
    map.insert("step-08-implement".to_string(), Tier::Fast);
    let outcome = stage(&f.run_dir, &[missing], &f.recipe(), &map)
        .expect("a missing search directory must not stop a run of 100 agent steps");
    assert!(outcome.notices.is_empty(), "{:?}", outcome.notices);
    assert!(
        outcome.staged.is_some(),
        "the recipe named on the command line is reached whatever the search path does"
    );
}

#[test]
fn the_top_level_recipe_is_staged_even_when_it_is_outside_the_search_directories() {
    // The documented fall-back if the staging directory cannot be placed
    // ahead of the bundled one: the override still covers the recipe named on
    // the command line.
    let f = Fixture::new();
    let elsewhere = f.run_dir.parent().unwrap().join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    let recipe = elsewhere.join("named.yaml");
    fs::write(&recipe, RECIPE).unwrap();
    let mut map: BTreeMap<String, Tier> = BTreeMap::new();
    map.insert("step-08-implement".to_string(), Tier::Fast);
    let staged = stage(&f.run_dir, &[], &recipe, &map)
        .expect("stage")
        .staged
        .expect("the recipe named on the command line is always reachable");
    let text = staged_text(&staged, "named.yaml");
    assert_eq!(
        model_of(&text, "step-08-implement").as_deref(),
        Some("fast")
    );
}
