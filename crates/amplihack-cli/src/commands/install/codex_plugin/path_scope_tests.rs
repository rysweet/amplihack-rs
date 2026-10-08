//! Frozen identities reject replacements even with identical journal bytes.
use super::super::codex_alias_fixture::AliasPaths;
use super::*;
use crate::test_support::{EnvGuard, home_env_lock};

fn replace(which: &str) {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let paths = AliasPaths::new(true);
    fs::create_dir_all(paths.root.join("market/.agents/plugins")).unwrap();
    fs::write(paths.root.join("pending.json"), b"identical").unwrap();
    let _env = EnvGuard::set([("HOME", paths.home.to_str().unwrap())]);
    let selected = match which {
        "home" => paths.home.clone(),
        "codex" => paths.codex_home.clone(),
        "parent" => paths.codex_home.parent().unwrap().to_path_buf(),
        "root" => paths.root.clone(),
        _ => paths.root.join("market"),
    };
    let held = paths.dir.path().join("held");
    let result = path_scope::with_scope(&paths.root, &paths.codex_home, || {
        fs::rename(&selected, &held)?;
        fs::create_dir(&selected)?;
        if which == "root" {
            fs::write(selected.join("pending.json"), b"identical")?;
        }
        // Nested recovery cannot capture the replacement as a new authority.
        path_scope::with_scope(&paths.root, &paths.codex_home, || Ok(()))
    });
    assert!(result.is_err(), "replacement must fail: {which}");
    assert!(held.is_dir());
    if which == "root" {
        assert_eq!(fs::read(held.join("pending.json")).unwrap(), b"identical");
        assert_eq!(
            fs::read(selected.join("pending.json")).unwrap(),
            b"identical"
        );
    }
    // A failed invocation clears its scope instead of leaking authority.
    assert!(path_scope::check().is_ok());
}
macro_rules! replacement {
    ($name:ident, $which:literal) => {
        #[test]
        fn $name() {
            replace($which);
        }
    };
}
replacement!(captured_selected_home_replacement_is_rejected, "home");
replacement!(captured_codex_home_replacement_is_rejected, "codex");
replacement!(captured_codex_parent_replacement_is_rejected, "parent");
replacement!(captured_staging_clone_replacement_is_rejected, "root");
replacement!(captured_market_replacement_is_rejected, "market");

#[test]
fn ordinary_child_creation_and_missing_descendant_adoption_preserve_scope() {
    let _lock = home_env_lock().lock().unwrap_or_else(|p| p.into_inner());
    let paths = AliasPaths::new(false);
    let _env = EnvGuard::set([("HOME", paths.home.to_str().unwrap())]);
    path_scope::with_scope(&paths.root, &paths.codex_home, || {
        fs::create_dir_all(paths.root.join("market/.agents/plugins"))?;
        fs::write(paths.home.join("ordinary-child"), b"child")?;
        path_scope::check()?;
        assert_eq!(
            path_scope::canonical(&paths.root)?,
            fs::canonicalize(&paths.root)?
        );
        let canonical_home = fs::canonicalize(&paths.home)?;
        for barrier in path_scope::ancestors(&paths.root.join("market"))? {
            assert!(barrier.starts_with(&canonical_home));
        }
        Ok(())
    })
    .unwrap();
    paths.assert_alias_unchanged();
}
