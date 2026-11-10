#[path = "/Users/ryan/src/factory-lanes/cx-1390/amplihack-rs/worktrees/feat/issue-1390-a-usage-limit-kills/tests/support/isolated_amplihack.rs"]
mod isolated_amplihack;
mod support {
    pub mod amplihack_bin {
        pub fn amplihack_bin() -> std::path::PathBuf {
            "/private/tmp/issue-1390-target/debug/amplihack".into()
        }
    }
}
mod simple;
mod complex;
