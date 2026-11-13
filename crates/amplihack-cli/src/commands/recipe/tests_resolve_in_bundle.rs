//! Issue #2528: `resolve_recipe_in_bundle` (D6, E9).
//!
//! Wire with, in `recipe/mod.rs`:
//! `#[cfg(all(test, unix))] mod tests_resolve_in_bundle;`
//!
//! Contract (docs/reference/recipe-run-bundle.md, "Resolving the recipe
//! argument"):
//! - A bare name resolves only in `selection.recipes_dir()`; the canonical
//!   result must stay inside it. Origin is `RecipeOrigin::Bundle`.
//! - An argument containing `/`, `\`, `..` or NUL, or ending in `.yaml` or
//!   `.yml`, is a path. Origin is `RecipeOrigin::File(<path>)`; it is never
//!   looked up in the bundle by name.
//! - E9 names the recipe, the bundle and the choice.
//!
//! These replace the repo-root-first expectations of the bare-name tests in
//! `recipe_tests.rs` for `recipe run`: the repository root is used only with
//! `BundleChoice::Checkout`. `recipe list` and `recipe show` keep
//! `recipe_search_dirs`.

use super::resolve::{RecipeOrigin, resolve_recipe_in_bundle};
use super::run::bundle_pin::{BundleChoice, SelectInputs, Selection, select_from};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

fn write_bundle(home: &Path, label: &str) -> PathBuf {
    let recipes = home.join("amplifier-bundle/recipes");
    fs::create_dir_all(&recipes).unwrap();
    fs::write(
        recipes.join("probe.yaml"),
        format!("name: probe\ndescription: {label}\nsteps: []\n"),
    )
    .unwrap();
    set_mode(&recipes.join("probe.yaml"), 0o644);
    set_mode(&recipes, 0o755);
    set_mode(&home.join("amplifier-bundle"), 0o755);
    recipes
}

struct Fixture {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    checkout: PathBuf,
    installed: PathBuf,
}

/// A checkout (with its own `probe` recipe, plus one in `.claude/recipes`)
/// and a separate installed home.
fn fixture() -> Fixture {
    let tmp = crate::test_support::tempdir_outside_framework_roots();
    let root = fs::canonicalize(tmp.path()).unwrap();
    let checkout = root.join("checkout");
    fs::create_dir_all(checkout.join(".git")).unwrap();
    write_bundle(&checkout, "checkout");
    let claude = checkout.join(".claude/recipes");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("probe.yaml"), "name: probe\nsteps: []\n").unwrap();
    let installed = root.join("installed");
    write_bundle(&installed, "installed");
    Fixture {
        _tmp: tmp,
        root,
        checkout,
        installed,
    }
}

fn selection(f: &Fixture, choice: Option<BundleChoice>) -> Selection {
    select_from(SelectInputs {
        choice,
        working_dir: &f.checkout,
        amplihack_home: Some(f.installed.as_os_str()),
        configured_home: None,
        user_home: None,
        run_bundle: None,
        run_bundle_choice: None,
    })
    .unwrap()
}

#[test]
fn bare_name_resolves_only_in_the_installed_bundle_by_default() {
    let f = fixture();
    let s = selection(&f, None);
    let (path, origin) = resolve_recipe_in_bundle("probe", &s, &f.checkout).unwrap();
    assert_eq!(
        path,
        fs::canonicalize(f.installed.join("amplifier-bundle/recipes/probe.yaml")).unwrap()
    );
    assert_eq!(origin, RecipeOrigin::Bundle);
}

#[test]
fn bare_name_resolves_in_the_checkout_only_when_asked() {
    let f = fixture();
    let s = selection(&f, Some(BundleChoice::Checkout));
    let (path, origin) = resolve_recipe_in_bundle("probe", &s, &f.checkout).unwrap();
    assert_eq!(
        path,
        fs::canonicalize(f.checkout.join("amplifier-bundle/recipes/probe.yaml")).unwrap()
    );
    assert_eq!(origin, RecipeOrigin::Bundle);
}

#[test]
fn a_bare_name_missing_from_the_bundle_is_e9_even_if_the_repo_has_it() {
    let f = fixture();
    // Only the checkout and its .claude/recipes have `only-here`.
    let only = f.checkout.join("amplifier-bundle/recipes/only-here.yaml");
    fs::write(&only, "name: only-here\nsteps: []\n").unwrap();
    let s = selection(&f, None);
    let err = resolve_recipe_in_bundle("only-here", &s, &f.checkout)
        .expect_err("the installed bundle has no only-here recipe");
    assert_eq!(
        format!("{err:#}"),
        format!(
            "recipe 'only-here' not found in the installed bundle at {}; \
             to use this checkout's recipes, pass --bundle checkout",
            s.source().display()
        )
    );
}

#[test]
fn e9_for_the_checkout_choice_names_the_checkout_bundle() {
    let f = fixture();
    let s = selection(&f, Some(BundleChoice::Checkout));
    let text = format!(
        "{:#}",
        resolve_recipe_in_bundle("nope", &s, &f.checkout).unwrap_err()
    );
    assert!(
        text.contains("recipe 'nope' not found in the checkout bundle at"),
        "{text}"
    );
    assert!(text.contains(&s.source().display().to_string()), "{text}");
}

#[test]
fn bare_name_symlinked_outside_the_bundle_is_refused() {
    let f = fixture();
    let outside = f.root.join("outside.yaml");
    fs::write(&outside, "name: evil\nsteps: []\n").unwrap();
    let recipes = f.installed.join("amplifier-bundle/recipes");
    symlink(&outside, recipes.join("evil.yaml")).unwrap();
    let s = selection(&f, None);
    let text = format!(
        "{:#}",
        resolve_recipe_in_bundle("evil", &s, &f.checkout).unwrap_err()
    );
    assert!(
        text.contains(&format!(
            "recipe 'evil' resolves outside {}",
            s.recipes_dir().display()
        )),
        "{text}"
    );
}

#[test]
fn a_path_argument_loads_the_given_file_and_records_it() {
    let f = fixture();
    let file = f.root.join("mine/custom.yaml");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "name: custom\nsteps: []\n").unwrap();
    let s = selection(&f, None);
    let arg = file.to_str().unwrap();
    let (path, origin) = resolve_recipe_in_bundle(arg, &s, &f.checkout).unwrap();
    assert_eq!(path, file);
    assert_eq!(origin, RecipeOrigin::File(file.clone()));
}

#[test]
fn path_like_arguments_are_never_looked_up_by_name() {
    let f = fixture();
    let s = selection(&f, None);
    for arg in [
        "probe.yaml",
        "probe.yml",
        "../recipes/probe",
        "..",
        "sub/probe",
        "sub\\probe",
        "probe\0",
    ] {
        // Not found as a path is fine; a lookup by name is not.
        if let Ok((_, origin)) = resolve_recipe_in_bundle(arg, &s, &f.checkout) {
            assert!(
                matches!(origin, RecipeOrigin::File(_)),
                "{arg:?} is a path, not a bundle name: {origin:?}"
            );
        }
    }
}
