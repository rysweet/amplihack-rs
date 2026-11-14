//! Issues #2528 and #2096 (PR #2290): a pinned run's runner arguments pass
//! through the run's execution checkout.
//!
//! Contract:
//! - `execute::map_search_args(&RunCheckout, Vec<OsString>) -> Result<Vec<OsString>>`
//!   sends the value after each `-R` and each `--agent-dir` through
//!   `RunCheckout::map_path` and keeps every other argument, in order.
//! - After mapping a value the toggle resets, so a value that is literally
//!   `-R` cannot shift the pairs.
//! - A mapping failure is returned, never replaced with the unmapped path.
//! - `runner_bundle_args` itself is unchanged; the pinned arms compose the two.

use super::execute::{map_search_args, runner_bundle_args};
use super::run_checkout::RunCheckout;
use crate::commands::recipe::resolve::RecipeOrigin;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const CALLER: &str = "/caller/repo";
const ROOT: &str = "/exec/checkout";

/// A private checkout that maps `/caller/repo/...` to `/exec/checkout/...`.
/// `released: true` makes its `Drop` a no-op, so nothing on disk is touched.
fn private() -> RunCheckout {
    RunCheckout::Private {
        exec_dir: PathBuf::from("/exec"),
        root: PathBuf::from(ROOT),
        caller_top: PathBuf::from(CALLER),
        repository: PathBuf::from("/exec/repository"),
        storage: PathBuf::from("/exec/storage"),
        owner: "test".to_string(),
        identities: [(0, 0); 3],
        released: true,
    }
}

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

#[test]
fn caller_checkout_leaves_every_argument_unchanged() {
    let args = os(&[
        "-R",
        "/caller/repo/amplifier-bundle/recipes",
        "--agent-dir",
        "/caller/repo/amplifier-bundle/agents",
        "--no-default-search",
    ]);
    let checkout = RunCheckout::Caller(PathBuf::from(CALLER));
    assert_eq!(map_search_args(&checkout, args.clone()).unwrap(), args);
}

#[test]
fn private_checkout_maps_search_dir_inside_caller_checkout() {
    let mapped = map_search_args(&private(), os(&["-R", "/caller/repo/recipes"])).unwrap();
    assert_eq!(mapped, os(&["-R", "/exec/checkout/recipes"]));
}

#[test]
fn private_checkout_maps_agent_dir_inside_caller_checkout() {
    let mapped = map_search_args(
        &private(),
        os(&["--agent-dir", "/caller/repo/amplifier-bundle/agents"]),
    )
    .unwrap();
    assert_eq!(
        mapped,
        os(&["--agent-dir", "/exec/checkout/amplifier-bundle/agents"])
    );
}

#[test]
fn snapshot_paths_and_flags_pass_through_in_order() {
    // A pinned snapshot lives under the state root, outside the caller's
    // checkout, so its paths are not rewritten. Flags are never mapped.
    let args = os(&[
        "-R",
        "/state/bundle-pins/k/amplifier-bundle/recipes",
        "-R",
        "/caller/repo/local-recipes",
        "--agent-dir",
        "/state/bundle-pins/k/amplifier-bundle/agents",
        "--no-default-search",
    ]);
    let mapped = map_search_args(&private(), args).unwrap();
    assert_eq!(
        mapped,
        os(&[
            "-R",
            "/state/bundle-pins/k/amplifier-bundle/recipes",
            "-R",
            "/exec/checkout/local-recipes",
            "--agent-dir",
            "/state/bundle-pins/k/amplifier-bundle/agents",
            "--no-default-search",
        ])
    );
}

#[test]
fn value_that_is_literally_dash_r_does_not_shift_the_pairs() {
    // The first `-R` takes the second as its value. The toggle must reset,
    // so the trailing flag stays a flag and is not mapped as a path.
    let mapped = map_search_args(&private(), os(&["-R", "-R", "--no-default-search"])).unwrap();
    assert_eq!(mapped.len(), 3);
    assert_eq!(mapped[0], OsString::from("-R"));
    assert_ne!(
        mapped[1],
        OsString::from("-R"),
        "value must go through map_path"
    );
    assert_eq!(mapped[2], OsString::from("--no-default-search"));
}

#[test]
fn trailing_flag_without_value_is_kept() {
    let mapped = map_search_args(&private(), os(&["--no-default-search", "-R"])).unwrap();
    assert_eq!(mapped, os(&["--no-default-search", "-R"]));
}

#[test]
fn empty_arguments_map_to_empty() {
    assert!(map_search_args(&private(), Vec::new()).unwrap().is_empty());
}

#[test]
fn mapping_failure_is_returned_not_replaced_with_unmapped_path() {
    let err = map_search_args(&private(), os(&["-R", "/caller/repo/../escape"]))
        .expect_err("a '..' path inside the caller checkout must fail closed");
    assert!(
        format!("{err:#}").contains(".."),
        "unexpected error: {err:#}"
    );

    let err = map_search_args(&private(), os(&["--agent-dir", "/caller/repo/a/../b"]))
        .expect_err("--agent-dir values fail closed the same way");
    assert!(
        format!("{err:#}").contains(".."),
        "unexpected error: {err:#}"
    );
}

#[test]
fn composed_with_runner_bundle_args_no_caller_path_reaches_the_runner() {
    // Auto-drive's invocation copy inside the caller's checkout, with a
    // recipe file there too: every path argument must land in the private
    // checkout, and `--no-default-search` must survive.
    let bundle = Path::new("/caller/repo/amplifier-bundle");
    let origin = RecipeOrigin::File(PathBuf::from("/caller/repo/recipes/x.yaml"));
    let mapped = map_search_args(&private(), runner_bundle_args(bundle, &origin)).unwrap();

    let mut expect_value = false;
    for arg in &mapped {
        let s = arg.to_str().unwrap();
        if expect_value {
            assert!(s.starts_with(ROOT), "unmapped path reached the runner: {s}");
            expect_value = false;
        } else if s == "-R" || s == "--agent-dir" {
            expect_value = true;
        }
        assert!(!s.starts_with(CALLER), "caller path leaked: {s}");
    }
    assert_eq!(mapped.last(), Some(&OsString::from("--no-default-search")));
    assert_eq!(
        mapped.len(),
        runner_bundle_args(bundle, &origin).len(),
        "mapping must not add or drop arguments"
    );
}
