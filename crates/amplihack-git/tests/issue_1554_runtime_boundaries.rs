//! Effective local exclusions and external placement, without user Git config.
#[path = "support/issue_1554.rs"]
mod fixture;

use amplihack_git::{launcher_state, runtime_preparation};
use fixture::{Repo, isolated, set, write};
use std::fs;

#[test]
fn later_negations_cannot_reexpose_runtime_in_either_checkout() {
    isolated(
        "later_negations_cannot_reexpose_runtime_in_either_checkout",
        || {
            let repo = Repo::new();
            let linked = repo.linked();
            let exclude = repo.root.join(".git/info/exclude");
            let original =
                b"# user rules stay intact\n/.amplihack/indexing.pid\n!/.amplihack/indexing.pid\n";
            write(&exclude, original);
            let before = repo.inventory(&repo.root);
            runtime_preparation::prepare(&linked).unwrap();
            runtime_preparation::prepare(&repo.root).unwrap();
            for checkout in [&repo.root, &linked] {
                write(checkout.join(".amplihack/indexing.pid"), "123\n");
                write(
                    checkout.join(".claude/runtime/sessions.jsonl.lock"),
                    "lock\n",
                );
                write(checkout.join(".amplihack/graph_db.wal"), "database log\n");
                assert_eq!(
                    repo.inventory(checkout),
                    before,
                    "runtime leaked in {}",
                    checkout.display()
                );
            }
            assert!(
                fs::read(&exclude).unwrap().starts_with(original),
                "existing exclusions were rewritten"
            );
            let once = fs::read(&exclude).unwrap();
            runtime_preparation::prepare(&linked).unwrap();
            assert_eq!(
                fs::read(exclude).unwrap(),
                once,
                "registration must be idempotent"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn substituted_clone_state_directory_is_rejected_before_writing_in_a_checkout() {
    isolated(
        "substituted_clone_state_directory_is_rejected_before_writing_in_a_checkout",
        || {
            let repo = Repo::new();
            let linked = repo.linked();
            let state_root = repo.temp.path().join("external");
            set("AMPLIHACK_RUNTIME_DIR", &state_root);
            let state = launcher_state::resolve(&repo.root).unwrap();
            fs::create_dir_all(&state_root).unwrap();
            // The base is external, but a computed descendant was substituted.
            std::os::unix::fs::symlink(&linked, state.directory.parent().unwrap()).unwrap();
            let before = repo.inventory(&linked);
            let result = launcher_state::prepare(&repo.root);
            assert!(
                result.is_err(),
                "checkout-contained resolved descendant accepted"
            );
            assert_eq!(
                repo.inventory(&linked),
                before,
                "preparation wrote through the symlink before rejecting it"
            );
            assert!(
                !linked.join(state.directory.file_name().unwrap()).exists(),
                "preparation created a directory through the substituted clone-state symlink"
            );
        },
    );
}

#[cfg(unix)]
#[test]
fn pre_existing_external_state_is_private_before_it_is_returned() {
    isolated(
        "pre_existing_external_state_is_private_before_it_is_returned",
        || {
            use std::os::unix::fs::PermissionsExt;
            let repo = Repo::new();
            set("AMPLIHACK_RUNTIME_DIR", repo.temp.path().join("external"));
            let state = launcher_state::resolve(&repo.root).unwrap();
            fs::create_dir_all(&state.directory).unwrap();
            fs::set_permissions(&state.directory, fs::Permissions::from_mode(0o755)).unwrap();
            if let Ok(prepared) = launcher_state::prepare(&repo.root) {
                assert_eq!(
                    fs::metadata(prepared.directory)
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o077,
                    0,
                    "preparation returned state readable by other users"
                );
            }
        },
    );
}
