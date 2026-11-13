//! Issue #2528: runner arguments and environment for a pinned run (D3, D5).
//!
//! Contract:
//! - `execute::build_search_dirs(&PinnedBundle, &RecipeOrigin) -> Vec<PathBuf>`:
//!   `<bundle>/recipes` first and, only for `RecipeOrigin::File`, the file's
//!   parent. Never the repository root, `.claude/recipes` or `~/.amplihack`.
//! - `execute::runner_bundle_args(&PinnedBundle, &RecipeOrigin) -> Vec<OsString>`:
//!   one `-R` per search dir, `--agent-dir <bundle>/agents` and
//!   `--no-default-search`.
//! - `execute::bundle_env(&PinnedBundle, state_home: &Path)
//!   -> Vec<(&'static str, OsString)>`: the four variables set on the runner
//!   process for an executing run. A dry-run pin yields none.
//! - `take_bundle_key(&mut BTreeMap<String, String>) -> Option<String>` in
//!   `run/mod.rs` removes the lower-case `bundle` key before export.
//! - `trusted::clean_retired_environment` leaves the four names in place (I3).

use super::bundle_pin::{SelectInputs, pin, select_from};
use super::execute::{build_search_dirs, bundle_env, runner_bundle_args};
use super::take_bundle_key;
use crate::commands::orch::autodrive::trusted::clean_retired_environment;
use crate::commands::recipe::resolve::RecipeOrigin;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const BUNDLE_VARS: [&str; 4] = [
    "AMPLIHACK_HOME",
    "AMPLIHACK_RUN_BUNDLE",
    "AMPLIHACK_RUN_BUNDLE_CHOICE",
    "AMPLIHACK_STATE_HOME",
];

struct Pinned {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    pinned: super::bundle_pin::PinnedBundle,
}

fn pinned(dry_run: bool) -> Pinned {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let home = root.join("installed");
    let bundle = home.join("amplifier-bundle");
    for dir in ["recipes", "agents"] {
        fs::create_dir_all(bundle.join(dir)).unwrap();
        fs::set_permissions(bundle.join(dir), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(
        bundle.join("recipes/probe.yaml"),
        "name: probe\nsteps: []\n",
    )
    .unwrap();
    fs::set_permissions(
        bundle.join("recipes/probe.yaml"),
        fs::Permissions::from_mode(0o644),
    )
    .unwrap();
    fs::set_permissions(&bundle, fs::Permissions::from_mode(0o755)).unwrap();
    // A checkout around the working directory, which must never be searched.
    let checkout = root.join("checkout");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    fs::create_dir_all(checkout.join("amplifier-bundle/recipes")).unwrap();
    let selection = select_from(SelectInputs {
        choice: None,
        working_dir: &checkout,
        amplihack_home: Some(home.as_os_str()),
        configured_home: None,
        user_home: None,
        run_bundle: None,
        run_bundle_choice: None,
    })
    .unwrap();
    let runtime = root.join("runs/r/runtime");
    fs::create_dir_all(&runtime).unwrap();
    let pinned = pin(&selection, &runtime, dry_run).unwrap();
    Pinned {
        _tmp: tmp,
        root,
        pinned,
    }
}

#[test]
fn search_dirs_for_a_bundle_recipe_are_the_snapshot_only() {
    let p = pinned(false);
    let dirs = build_search_dirs(&p.pinned, &RecipeOrigin::Bundle);
    assert_eq!(dirs, vec![p.pinned.bundle().join("recipes")]);
    assert!(dirs[0].starts_with(p.root.join("runs/r/runtime/bundle")));
}

#[test]
fn search_dirs_for_a_file_recipe_add_its_parent_second() {
    let p = pinned(false);
    let file = p.root.join("mine/custom.yaml");
    let dirs = build_search_dirs(&p.pinned, &RecipeOrigin::File(file.clone()));
    assert_eq!(
        dirs,
        vec![
            p.pinned.bundle().join("recipes"),
            file.parent().unwrap().to_path_buf()
        ]
    );
}

#[test]
fn search_dirs_for_a_file_inside_the_bundle_are_not_duplicated() {
    let p = pinned(false);
    let file = p.pinned.bundle().join("recipes/probe.yaml");
    let dirs = build_search_dirs(&p.pinned, &RecipeOrigin::File(file));
    assert_eq!(dirs, vec![p.pinned.bundle().join("recipes")]);
}

#[test]
fn a_dry_run_points_the_runner_at_the_source_bundle() {
    let p = pinned(true);
    let dirs = build_search_dirs(&p.pinned, &RecipeOrigin::Bundle);
    assert_eq!(dirs, vec![p.pinned.source().join("recipes")]);
    assert!(
        bundle_env(&p.pinned, &p.root.join("state")).is_empty(),
        "a dry run sets none of the four variables"
    );
}

#[test]
fn runner_arguments_turn_default_search_off_and_name_the_agents_dir() {
    let p = pinned(false);
    let args = runner_bundle_args(&p.pinned, &RecipeOrigin::Bundle);
    let recipes = p.pinned.bundle().join("recipes");
    let agents = p.pinned.bundle().join("agents");
    let expected: Vec<OsString> = vec![
        "-R".into(),
        recipes.into_os_string(),
        "--agent-dir".into(),
        agents.into_os_string(),
        "--no-default-search".into(),
    ];
    assert_eq!(args, expected);
}

#[test]
fn bundle_env_points_steps_at_the_snapshot() {
    let p = pinned(false);
    let state = p.root.join("state");
    let env: BTreeMap<&str, OsString> = bundle_env(&p.pinned, &state).into_iter().collect();
    let home = p.pinned.home().as_os_str();
    assert_eq!(env.len(), 4, "{env:?}");
    assert_eq!(env["AMPLIHACK_HOME"], home);
    assert_eq!(env["AMPLIHACK_RUN_BUNDLE"], home);
    assert_eq!(env["AMPLIHACK_RUN_BUNDLE_CHOICE"], OsStr::new("installed"));
    assert_eq!(env["AMPLIHACK_STATE_HOME"], state.as_os_str());
    assert!(
        !env.contains_key("AMPLIHACK_BUNDLE_DIR"),
        "the retired name is never set"
    );
}

#[test]
fn clean_retired_environment_keeps_the_bundle_variables() {
    let mut command = std::process::Command::new("true");
    for name in BUNDLE_VARS {
        command.env(name, "/runs/r/runtime/bundle");
    }
    command.env("AMPLIHACK_BUNDLE_DIR", "/retired");
    clean_retired_environment(&mut command).unwrap();
    let envs: BTreeMap<&OsStr, Option<&OsStr>> = command.get_envs().collect();
    for name in BUNDLE_VARS {
        assert_eq!(
            envs.get(OsStr::new(name)),
            Some(&Some(OsStr::new("/runs/r/runtime/bundle"))),
            "{name} must survive clean_retired_environment"
        );
    }
    assert_eq!(envs.get(OsStr::new("AMPLIHACK_BUNDLE_DIR")), Some(&None));
}

#[test]
fn the_bundle_key_is_taken_out_of_the_context_before_export() {
    let mut context = BTreeMap::from([
        ("bundle".to_string(), "checkout".to_string()),
        ("task_description".to_string(), "x".to_string()),
        (
            "Bundle".to_string(),
            "kept: keys are case-sensitive".to_string(),
        ),
    ]);
    assert_eq!(take_bundle_key(&mut context).as_deref(), Some("checkout"));
    assert!(!context.contains_key("bundle"));
    assert!(context.contains_key("task_description"));
    assert!(context.contains_key("Bundle"));
    assert_eq!(take_bundle_key(&mut context), None);
}

#[test]
fn bundle_variables_are_reserved_context_keys() {
    for name in BUNDLE_VARS {
        let context = BTreeMap::from([(name.to_string(), "/x".to_string())]);
        let mut command = std::process::Command::new("true");
        let err = super::execute::pass_context(&mut command, &context)
            .expect_err("AMPLIHACK_* keys are reserved");
        assert!(format!("{err:#}").contains("reserved"), "{name}: {err:#}");
    }
}

// ---- clap: `recipe run --bundle` ---------------------------------------

fn parse_bundle(args: &[&str]) -> Result<Option<String>, clap::Error> {
    let mut argv = vec!["amplihack", "recipe", "run", "default-workflow"];
    argv.extend_from_slice(args);
    let cli = crate::Cli::try_parse_from(argv)?;
    match cli.command {
        crate::Commands::Recipe {
            command: crate::RecipeCommands::Run { bundle, .. },
        } => Ok(bundle),
        other => panic!("expected recipe run, got {other:?}"),
    }
}

#[test]
fn bundle_flag_accepts_the_two_choices_and_defaults_to_none() {
    assert_eq!(parse_bundle(&[]).unwrap(), None);
    assert_eq!(
        parse_bundle(&["--bundle", "installed"]).unwrap().as_deref(),
        Some("installed")
    );
    assert_eq!(
        parse_bundle(&["--bundle=checkout"]).unwrap().as_deref(),
        Some("checkout")
    );
}

#[test]
fn bundle_flag_rejects_any_other_value() {
    for bad in ["Checkout", "/some/path", ""] {
        assert!(parse_bundle(&["--bundle", bad]).is_err(), "{bad:?}");
    }
}

#[test]
fn the_bundle_context_key_is_not_a_removed_key() {
    let mut out = Vec::new();
    crate::commands::recipe::reject_removed_context_keys(&["bundle=checkout".into()], &mut out)
        .expect("`-c bundle=` is accepted");
    assert!(out.is_empty());
}
