//! Private file-protocol transport setup for the finalisation fixtures.

use super::Fixture;

pub(super) fn prepare_repository_rewrite(fx: &Fixture) {
    let output = amplihack_git::command_in(&fx.repo)
        .args([
            "config",
            &format!("url.{}/.insteadOf", fx.tmp.path().join("remotes").display()),
            "https://github.com/",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ALLOW_PROTOCOL", "file")
        .output()
        .expect("configure the owned fixture repository");
    assert!(
        output.status.success(),
        "fixture URL rewrite: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
