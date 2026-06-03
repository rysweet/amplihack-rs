mod build_install_command;
mod classify_skip_reason;
mod network;
mod skip_line;
mod subcommand_routing;

/// RAII guard that captures and clears env vars that influence skip-check
/// decisions, restoring them on drop. Shared across test sub-modules.
pub(super) struct SkipSignalEnvGuard {
    prev: Vec<(&'static str, Option<std::ffi::OsString>)>,
}

impl SkipSignalEnvGuard {
    pub(super) fn capture_and_clear() -> Self {
        let names: &[&'static str] = &[
            "AMPLIHACK_NONINTERACTIVE",
            "AMPLIHACK_AGENT_BINARY",
            "AMPLIHACK_NO_UPDATE_CHECK",
            "AMPLIHACK_PARITY_TEST",
            "CI",
        ];
        let prev: Vec<(&'static str, Option<std::ffi::OsString>)> =
            names.iter().map(|n| (*n, std::env::var_os(n))).collect();
        unsafe {
            for (name, _) in &prev {
                std::env::remove_var(name);
            }
        }
        Self { prev }
    }
}

impl Drop for SkipSignalEnvGuard {
    fn drop(&mut self) {
        unsafe {
            for (name, value) in self.prev.drain(..) {
                match value {
                    Some(v) => std::env::set_var(name, v),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
