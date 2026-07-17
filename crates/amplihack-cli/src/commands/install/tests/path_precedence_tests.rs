use super::paths::ensure_local_bin_on_shell_path;
use std::fs;

#[test]
fn shell_profile_gets_managed_prepend_even_when_local_bin_already_appears_later() {
    let _lock = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());
    let previous_shell = std::env::var_os("SHELL");
    unsafe {
        std::env::set_var("SHELL", "/bin/bash");
    }
    let bashrc = temp.path().join(".bashrc");
    fs::write(
        &bashrc,
        "export PATH=\"/opt/stale-python:$PATH:$HOME/.local/bin\"\n",
    )
    .unwrap();

    ensure_local_bin_on_shell_path().unwrap();

    let content = fs::read_to_string(&bashrc).unwrap();
    match previous_shell {
        Some(value) => unsafe { std::env::set_var("SHELL", value) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
    assert!(
        content.contains("# >>> amplihack managed PATH >>>")
            && content.contains("# <<< amplihack managed PATH <<<"),
        "future-shell PATH repair must use an idempotent managed block, got:\n{content}"
    );
    assert!(
        content.contains("export PATH=\"$HOME/.local/bin:$PATH\""),
        "managed block must prepend ~/.local/bin rather than treating a later mention as sufficient:\n{content}"
    );
}

#[test]
fn shell_profile_managed_prepend_is_idempotent() {
    let _lock = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());
    let previous_shell = std::env::var_os("SHELL");
    unsafe {
        std::env::set_var("SHELL", "/bin/bash");
    }

    ensure_local_bin_on_shell_path().unwrap();
    ensure_local_bin_on_shell_path().unwrap();

    let content = fs::read_to_string(temp.path().join(".bashrc")).unwrap();
    match previous_shell {
        Some(value) => unsafe { std::env::set_var("SHELL", value) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
    assert_eq!(
        content.matches("# >>> amplihack managed PATH >>>").count(),
        1,
        "managed PATH block must not be duplicated:\n{content}"
    );
}

#[test]
fn shell_profile_existing_managed_block_is_moved_after_later_path_mutations() {
    let _lock = crate::test_support::home_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let _home = crate::test_support::HomeGuard::set(temp.path());
    let previous_shell = std::env::var_os("SHELL");
    unsafe {
        std::env::set_var("SHELL", "/bin/bash");
    }
    let bashrc = temp.path().join(".bashrc");
    fs::write(
        &bashrc,
        "# >>> amplihack managed PATH >>>\nexport PATH=\"$HOME/.local/bin:$PATH\"\n# <<< amplihack managed PATH <<<\nexport PATH=\"/opt/stale-python:$PATH\"\n",
    )
    .unwrap();

    ensure_local_bin_on_shell_path().unwrap();

    let content = fs::read_to_string(&bashrc).unwrap();
    match previous_shell {
        Some(value) => unsafe { std::env::set_var("SHELL", value) },
        None => unsafe { std::env::remove_var("SHELL") },
    }
    let stale_pos = content
        .find("/opt/stale-python")
        .expect("test fixture must keep stale PATH mutation");
    let managed_pos = content
        .rfind("# >>> amplihack managed PATH >>>")
        .expect("managed PATH block must remain");
    assert!(
        managed_pos > stale_pos,
        "managed PATH block must be moved after later PATH mutations so it wins in future shells:\n{content}"
    );
    assert_eq!(
        content.matches("# >>> amplihack managed PATH >>>").count(),
        1,
        "managed PATH block must still be singular after move:\n{content}"
    );
}
