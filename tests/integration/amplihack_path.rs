//! `PATH` for scripts that call a bare `amplihack`.
//!
//! Recipe steps and helper scripts call `amplihack ...` by name. A test that runs
//! one with the ambient `PATH` resolves that name to whatever `amplihack` happens to
//! be installed: none on a clean CI runner, a stale build on a developer's machine.
//! Such a test then passes or fails with the order other tests ran in, not with this
//! tree's binary. Give those scripts this `PATH` instead.

/// The ambient `PATH` with the directory of the `amplihack` binary under test first.
pub(crate) fn path_with_amplihack_bin() -> String {
    let dir = std::path::Path::new(env!("CARGO_BIN_EXE_amplihack"))
        .parent()
        .expect("the amplihack binary has a parent directory")
        .to_string_lossy()
        .into_owned();
    match std::env::var("PATH") {
        Ok(path) if !path.is_empty() => format!("{dir}:{path}"),
        _ => dir,
    }
}
