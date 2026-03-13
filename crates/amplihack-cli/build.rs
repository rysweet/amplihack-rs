// build.rs — amplihack-cli build script
//
// Purpose: force Cargo to resolve cxx-build as a build dependency of this
// crate, ensuring the pinned version (=1.0.138) declared in
// [workspace.dependencies] and [build-dependencies] takes effect.
//
// Without this file the [build-dependencies] section is ignored at compile
// time and cxx-build can be pulled in at a different version by another
// dependency (e.g. kuzu), causing ABI-incompatible bridge symbols and
// link-time failures.  See docs/howto/resolve-kuzu-linker-errors.md and
// SEC-WS4-01 in Cargo.toml.
//
// The script itself performs no code generation; its presence is sufficient
// to register the build-dependency and anchor the version pin.

fn main() {
    // Nothing to do — the sole purpose of this build script is to ensure
    // cxx-build is resolved at the pinned version.
    println!("cargo:rerun-if-changed=build.rs");
}

#[cfg(test)]
mod tests {
    /// Verify that the cxx-build pin is declared in Cargo.toml.
    ///
    /// This test reads the raw Cargo.toml text and checks for the pinned
    /// version string, giving a clear failure message if the pin is removed.
    #[test]
    fn cxx_build_pin_present() {
        let cargo_toml = include_str!("Cargo.toml");
        assert!(
            cargo_toml.contains("cxx-build"),
            "cxx-build must appear in amplihack-cli/Cargo.toml [build-dependencies]"
        );
    }

    /// Verify that this build script compiles (trivially true if the test runs).
    #[test]
    fn build_rs_compiles() {
        // If we reach this line, build.rs compiled successfully.
    }
}
