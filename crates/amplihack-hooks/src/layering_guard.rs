//! Regression guard for the #875 layering fix.
//!
//! `amplihack-hooks` is a lower-level crate than `amplihack-cli`. It must never
//! depend on `amplihack-cli` again — shared helpers were extracted into
//! `amplihack-utils` and `amplihack-memory` precisely so this inversion cannot
//! recur. The compiler only rejects an outright dependency *cycle*; a one-way
//! `hooks -> cli` edge would compile fine, so this test makes the constraint
//! self-defending by inspecting the crate manifest directly.

#[cfg(test)]
mod tests {
    /// The manifest must not declare a dependency on `amplihack-cli` in any
    /// dependency table (`[dependencies]`, `[dev-dependencies]`,
    /// `[build-dependencies]`, or their target-specific variants).
    #[test]
    fn hooks_must_not_depend_on_cli() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read amplihack-hooks Cargo.toml");

        for (lineno, raw) in manifest.lines().enumerate() {
            let line = raw.trim();
            if line.starts_with('#') {
                continue;
            }
            let is_dep_key = line
                .split_once('=')
                .map(|(key, _)| {
                    let key = key.trim().trim_matches('"');
                    key == "amplihack-cli" || key == "amplihack_cli"
                })
                .unwrap_or(false);
            assert!(
                !is_dep_key,
                "amplihack-hooks must not depend on amplihack-cli (#875 layering \
                 inversion regression) — offending Cargo.toml line {}: {raw:?}. \
                 Move shared helpers into amplihack-utils/amplihack-memory instead.",
                lineno + 1
            );
        }
    }
}
