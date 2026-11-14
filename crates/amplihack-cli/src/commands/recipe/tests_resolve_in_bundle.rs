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
//!   `.yml`, is a path; it is never looked up in the bundle by name. It is
//!   canonicalised first: a path that cannot be, or that is not a regular
//!   file, is E15. Then (B1):
//!   - under the selected bundle's `recipes/`: `(canonical, Bundle)`, so the
//!     run takes the recipe, its sub-recipes and its tools from one bundle;
//!   - under any other `amplifier-bundle/recipes/`: E14, whose advice depends
//!     on whether that bundle sits in a git checkout;
//!   - anywhere else: `(path, File(path))`, a standalone recipe.
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
        work_root: None,
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
        // Not found as a path is fine (E15); a lookup by name is not. None
        // of these names a file under the selected bundle's recipes/, so a
        // success can only be a standalone file.
        if let Ok((_, origin)) = resolve_recipe_in_bundle(arg, &s, &f.checkout) {
            assert!(
                matches!(origin, RecipeOrigin::File(_)),
                "{arg:?} is a path, not a bundle name: {origin:?}"
            );
        }
    }
}

// ------------------------------------------------------ paths (B1, E14, E15)

fn err(recipe: &str, s: &Selection, working_dir: &Path) -> String {
    format!(
        "{:#}",
        resolve_recipe_in_bundle(recipe, s, working_dir).expect_err("expected an error")
    )
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// E14 when the other bundle sits in a git checkout at `checkout`.
fn e14_checkout(path: &Path, checkout: &Path, s: &Selection, name: &str) -> String {
    format!(
        "recipe {} belongs to the bundle at {}, not to this run's {} bundle at {}; \
         to use that bundle, run from inside {} with --bundle checkout: \
         amplihack recipe run {name} --bundle checkout; \
         or run this run's bundle by name: amplihack recipe run {name}",
        path.display(),
        checkout.join("amplifier-bundle").display(),
        s.choice().as_str(),
        s.source().display(),
        checkout.display(),
    )
}

#[test]
fn a_path_into_the_selected_installed_bundle_runs_from_that_bundle() {
    let f = fixture();
    let s = selection(&f, None);
    let file = f.installed.join("amplifier-bundle/recipes/probe.yaml");
    let (path, origin) = resolve_recipe_in_bundle(arg(&file), &s, &f.checkout).unwrap();
    assert_eq!(path, fs::canonicalize(&file).unwrap());
    assert_eq!(origin, RecipeOrigin::Bundle);
}

#[test]
fn a_relative_path_into_the_selected_checkout_bundle_runs_from_that_bundle() {
    let f = fixture();
    let s = selection(&f, Some(BundleChoice::Checkout));
    let (path, origin) =
        resolve_recipe_in_bundle("amplifier-bundle/recipes/probe.yaml", &s, &f.checkout).unwrap();
    assert_eq!(
        path,
        fs::canonicalize(f.checkout.join("amplifier-bundle/recipes/probe.yaml")).unwrap()
    );
    assert_eq!(origin, RecipeOrigin::Bundle);
}

#[test]
fn a_checkout_path_with_the_installed_bundle_is_e14_naming_both_bundles() {
    let f = fixture();
    let s = selection(&f, None);
    let text = err("amplifier-bundle/recipes/probe.yaml", &s, &f.checkout);
    let canonical =
        fs::canonicalize(f.checkout.join("amplifier-bundle/recipes/probe.yaml")).unwrap();
    assert_eq!(text, e14_checkout(&canonical, &f.checkout, &s, "probe"));
}

#[test]
fn a_dot_dot_path_that_canonicalises_into_the_other_bundle_is_e14() {
    let f = fixture();
    let s = selection(&f, None);
    // Starts in the installed bundle, then climbs into the checkout's.
    let sneaky = f
        .installed
        .join("amplifier-bundle/recipes/../../../checkout/amplifier-bundle/recipes/probe.yaml");
    let text = err(arg(&sneaky), &s, &f.checkout);
    let canonical =
        fs::canonicalize(f.checkout.join("amplifier-bundle/recipes/probe.yaml")).unwrap();
    assert_eq!(text, e14_checkout(&canonical, &f.checkout, &s, "probe"));
}

#[test]
fn a_symlinked_recipe_in_the_selected_bundle_pointing_at_the_other_is_e14() {
    let f = fixture();
    let s = selection(&f, None);
    let link = f.installed.join("amplifier-bundle/recipes/borrowed.yaml");
    symlink(
        f.checkout.join("amplifier-bundle/recipes/probe.yaml"),
        &link,
    )
    .unwrap();
    let text = err(arg(&link), &s, &f.checkout);
    assert!(text.contains("belongs to the bundle at"), "{text}");
    assert!(
        text.contains(&format!(
            "{}, not to this run's",
            f.checkout.join("amplifier-bundle").display()
        )),
        "{text}"
    );
}

#[test]
fn e14_names_a_yml_recipe_by_its_stem() {
    let f = fixture();
    let other = f.checkout.join("amplifier-bundle/recipes/other.yml");
    fs::write(&other, "name: other\nsteps: []\n").unwrap();
    let s = selection(&f, None);
    let text = err(arg(&other), &s, &f.checkout);
    let canonical = fs::canonicalize(&other).unwrap();
    assert_eq!(text, e14_checkout(&canonical, &f.checkout, &s, "other"));
}

#[test]
fn a_path_into_another_worktree_is_e14_and_says_to_run_from_that_worktree() {
    let f = fixture();
    // A linked worktree: `.git` is a file, which is not parsed.
    let worktree = f.root.join("worktree");
    fs::create_dir_all(&worktree).unwrap();
    fs::write(worktree.join(".git"), "gitdir: /nowhere\n").unwrap();
    write_bundle(&worktree, "worktree");
    for choice in [None, Some(BundleChoice::Checkout)] {
        let s = selection(&f, choice);
        let file = worktree.join("amplifier-bundle/recipes/probe.yaml");
        let text = err(arg(&file), &s, &f.checkout);
        assert_eq!(
            text,
            e14_checkout(&fs::canonicalize(&file).unwrap(), &worktree, &s, "probe"),
            "{choice:?}"
        );
    }
}

#[test]
fn a_path_into_the_installed_bundle_with_the_checkout_choice_is_e14_without_checkout_advice() {
    let f = fixture();
    let s = selection(&f, Some(BundleChoice::Checkout));
    let file = f.installed.join("amplifier-bundle/recipes/probe.yaml");
    let text = err(arg(&file), &s, &f.checkout);
    assert_eq!(
        text,
        format!(
            "recipe {} belongs to the bundle at {}, not to this run's checkout bundle at {}; \
             --bundle checkout cannot select that bundle because it is not in a git checkout; \
             run the recipe by name: amplihack recipe run probe \
             (add --bundle installed for the installed bundle)",
            fs::canonicalize(&file).unwrap().display(),
            f.installed.join("amplifier-bundle").display(),
            s.source().display(),
        )
    );
}

#[test]
fn a_missing_path_is_e15_naming_the_path_as_given() {
    let f = fixture();
    let s = selection(&f, None);
    for given in ["amplifier-bundle/recipes/absent.yaml", "mine/absent.yaml"] {
        let text = err(given, &s, &f.checkout);
        assert!(
            text.starts_with(&format!("recipe {given} not found: ")),
            "{given}: {text}"
        );
    }
    let absolute = f.root.join("nowhere/absent.yaml");
    let text = err(arg(&absolute), &s, &f.checkout);
    assert!(
        text.starts_with(&format!("recipe {} not found: ", absolute.display())),
        "{text}"
    );
}

#[test]
fn a_directory_is_e15_even_under_the_selected_recipes_dir() {
    let f = fixture();
    let s = selection(&f, None);
    let dir = f.installed.join("amplifier-bundle/recipes/nested.yaml");
    fs::create_dir_all(&dir).unwrap();
    let text = err(arg(&dir), &s, &f.checkout);
    assert_eq!(
        text,
        format!("recipe {} is not a regular file", dir.display())
    );
    // The same in another bundle: E15, not E14.
    let other = f.checkout.join("amplifier-bundle/recipes");
    let text = err(arg(&other), &s, &f.checkout);
    assert_eq!(
        text,
        format!("recipe {} is not a regular file", other.display())
    );
}

#[test]
fn paths_outside_any_bundle_recipes_dir_stay_standalone_files() {
    let f = fixture();
    let s = selection(&f, None);
    for rel in [
        "recipes/x.yaml",
        "amplifier-bundle/recipes-old/x.yaml",
        "amplifier-bundle/tools/x.yaml",
        "not-amplifier-bundle/recipes/x.yaml",
    ] {
        let file = f.root.join("standalone").join(rel);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, "name: x\nsteps: []\n").unwrap();
        let (path, origin) = resolve_recipe_in_bundle(arg(&file), &s, &f.checkout)
            .unwrap_or_else(|e| panic!("{rel}: {e:#}"));
        assert_eq!(path, file, "{rel}");
        assert_eq!(origin, RecipeOrigin::File(file.clone()), "{rel}");
    }
}

#[test]
fn a_path_reached_through_a_symlinked_directory_is_classified_by_its_target() {
    let f = fixture();
    let s = selection(&f, None);
    let alias = f.root.join("alias");
    symlink(&f.installed, &alias).unwrap();
    let file = alias.join("amplifier-bundle/recipes/probe.yaml");
    let (path, origin) = resolve_recipe_in_bundle(arg(&file), &s, &f.checkout).unwrap();
    assert_eq!(path, fs::canonicalize(&file).unwrap());
    assert_eq!(origin, RecipeOrigin::Bundle);
}

#[test]
fn a_release_link_install_accepts_paths_through_the_link_and_the_target() {
    // `<root>/release -> versions/1`, the layout `amplihack install` uses.
    let f = fixture();
    let real = f.root.join("versions/1");
    write_bundle(&real, "release");
    let link = f.root.join("release");
    symlink(&real, &link).unwrap();
    let s = select_from(SelectInputs {
        choice: None,
        working_dir: &f.checkout,
        amplihack_home: Some(link.as_os_str()),
        configured_home: None,
        user_home: None,
        run_bundle: None,
        run_bundle_choice: None,
        work_root: None,
    })
    .unwrap();
    for home in [&link, &real] {
        let file = home.join("amplifier-bundle/recipes/probe.yaml");
        let (path, origin) = resolve_recipe_in_bundle(arg(&file), &s, &f.checkout)
            .unwrap_or_else(|e| panic!("{}: {e:#}", file.display()));
        assert_eq!(path, fs::canonicalize(&file).unwrap());
        assert_eq!(origin, RecipeOrigin::Bundle, "{}", file.display());
    }
}
