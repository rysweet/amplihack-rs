use crate::support::KEY;
use lbug_cache::{
    config::{ManagedMode, managed_mode},
    selection::SelectionState,
};
use std::collections::BTreeMap;

#[test]
fn any_explicit_upstream_override_including_empty_values_opts_out() {
    assert_eq!(managed_mode(&BTreeMap::new()), ManagedMode::Auto);
    for name in ["LBUG_SHARED", "LBUG_LIBRARY_DIR", "LBUG_INCLUDE_DIR"] {
        for value in ["", "0", "/owned/explicit"] {
            let env = BTreeMap::from([(name.into(), value.into())]);
            assert_eq!(managed_mode(&env), ManagedMode::Upstream, "{name}={value}");
        }
    }
    let env = BTreeMap::from([("AMPLIHACK_LBUG_CACHE".into(), "source".into())]);
    assert_eq!(managed_mode(&env), ManagedMode::Source);
    let env = BTreeMap::from([("AMPLIHACK_LBUG_CACHE".into(), "typo".into())]);
    assert_eq!(managed_mode(&env), ManagedMode::Source);
}

#[test]
fn integrity_rejection_vetoes_repair_for_the_entire_invocation() {
    let mut selection = SelectionState::new("invocation-one");
    assert!(selection.accept_verified(KEY));
    let managed_input = selection.build_script_input();
    selection.reject("checksum_mismatch");
    assert!(
        !selection.accept_verified(KEY),
        "redownload must not clear a rejection"
    );
    assert!(!selection.accept_verified("lbug-0.15.5-aarch64-apple-darwin-appleclang17"));
    assert!(selection.requires_source());
    assert_ne!(
        selection.build_script_input(),
        managed_input,
        "warm Cargo output must be invalidated"
    );
    assert!(SelectionState::new("invocation-two").accept_verified(KEY));
}

#[test]
fn absent_unreadable_or_wrong_invocation_records_invalidate_managed_selection() {
    let mut original = SelectionState::new("original");
    assert!(original.accept_verified(KEY));
    let input = original.build_script_input();
    for record in [None, Some(b"not json".as_slice()), Some(b"{}".as_slice())] {
        let state = SelectionState::restore("original", record);
        assert!(state.requires_source());
        assert_ne!(state.build_script_input(), input);
    }
    let bytes = original.to_record();
    assert!(SelectionState::restore("different", Some(&bytes)).requires_source());
}

#[test]
fn successful_source_output_survives_packaging_failure_without_recompilation() {
    use lbug_cache::producer::build_and_package;
    use std::cell::Cell;
    let calls = Cell::new(0);
    let output = build_and_package(
        || {
            calls.set(calls.get() + 1);
            Ok::<_, String>("native output from requesting build".to_owned())
        },
        |_| Err::<(), _>("publication unavailable".to_owned()),
    )
    .unwrap();
    assert_eq!(output.native, "native output from requesting build");
    assert!(output.artifact.is_err());
    assert_eq!(calls.get(), 1);
}

#[test]
fn producing_miss_packages_the_exact_requesting_build_output_once() {
    use lbug_cache::producer::build_and_package;
    use std::cell::Cell;
    let compilations = Cell::new(0);
    let packages = Cell::new(0);
    let result = build_and_package(
        || {
            compilations.set(compilations.get() + 1);
            Ok::<_, String>(vec![1_u8, 9, 1, 7])
        },
        |native| {
            packages.set(packages.get() + 1);
            assert_eq!(native, &[1_u8, 9, 1, 7]);
            Ok::<_, String>("prepared bundle")
        },
    )
    .unwrap();
    assert_eq!(result.native, [1_u8, 9, 1, 7]);
    assert_eq!(result.artifact.unwrap(), "prepared bundle");
    assert_eq!((compilations.get(), packages.get()), (1, 1));
}

#[test]
fn source_failure_is_preserved_and_packaging_never_runs() {
    use lbug_cache::producer::build_and_package;
    let result = build_and_package(
        || Err::<String, _>("compiler failed".to_owned()),
        |_| -> Result<(), String> { panic!("cannot package a failed source build") },
    );
    assert_eq!(result.unwrap_err(), "compiler failed");
}
