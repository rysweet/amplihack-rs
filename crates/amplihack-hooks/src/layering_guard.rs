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
    use toml::Value;

    const FORBIDDEN: [&str; 2] = ["amplihack-cli", concat!("amplihack", "_", "cli")];

    /// True if `deps` (a `[dependencies]`-style table) declares an edge onto
    /// `amplihack-cli`, either by the dependency key itself or via an explicit
    /// `package = "amplihack-cli"` rename.
    fn table_depends_on_cli(deps: &Value) -> Option<String> {
        let table = deps.as_table()?;
        for (name, spec) in table {
            if FORBIDDEN.contains(&name.as_str()) {
                return Some(name.clone());
            }
            // Renamed dependency: `alias = { package = "amplihack-cli", .. }`.
            if let Some(pkg) = spec.get("package").and_then(Value::as_str)
                && FORBIDDEN.contains(&pkg)
            {
                return Some(format!("{name} (package = {pkg:?})"));
            }
        }
        None
    }

    /// Walk every dependency table in a manifest fragment: the top-level
    /// `[dependencies]`, `[dev-dependencies]`, `[build-dependencies]`, and any
    /// nested tables (e.g. the per-triple tables under `[target.*]`).
    fn find_cli_edge(node: &Value, path: &str) -> Option<String> {
        let table = node.as_table()?;
        for (key, value) in table {
            match key.as_str() {
                "dependencies" | "dev-dependencies" | "build-dependencies" => {
                    if let Some(hit) = table_depends_on_cli(value) {
                        return Some(format!("{path}[{key}] -> {hit}"));
                    }
                }
                // Recurse through `[target]` (and any other) sub-tables so
                // target-specific dependency tables are covered too.
                _ => {
                    if let Some(hit) = find_cli_edge(value, &format!("{path}{key}.")) {
                        return Some(hit);
                    }
                }
            }
        }
        None
    }

    /// The manifest must not declare a dependency on `amplihack-cli` in any
    /// dependency table (`[dependencies]`, `[dev-dependencies]`,
    /// `[build-dependencies]`, or their target-specific variants), whether by
    /// name or via a `package = "amplihack-cli"` rename.
    #[test]
    fn hooks_must_not_depend_on_cli() {
        let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
            .expect("read amplihack-hooks Cargo.toml");
        let parsed: Value = manifest
            .parse()
            .expect("amplihack-hooks Cargo.toml is valid TOML");

        if let Some(edge) = find_cli_edge(&parsed, "") {
            panic!(
                "amplihack-hooks must not depend on amplihack-cli (#875 layering \
                 inversion regression) — offending edge: {edge}. Move shared \
                 helpers into amplihack-utils/amplihack-memory instead."
            );
        }
    }

    #[test]
    fn detects_inline_dependency() {
        let m: Value = "[dependencies]\namplihack-cli = \"1\"\n".parse().unwrap();
        assert!(find_cli_edge(&m, "").is_some());
    }

    #[test]
    fn detects_dotted_table_dependency() {
        // `[dependencies.amplihack-cli]` table form — the old line-scan missed this.
        let m: Value = "[dependencies.amplihack-cli]\nworkspace = true\n"
            .parse()
            .unwrap();
        assert!(find_cli_edge(&m, "").is_some());
    }

    #[test]
    fn detects_renamed_package_dependency() {
        // Alias whose `package` points at amplihack-cli — also missed before.
        let m: Value = "[dependencies]\nfoo = { package = \"amplihack-cli\", version = \"1\" }\n"
            .parse()
            .unwrap();
        assert!(find_cli_edge(&m, "").is_some());
    }

    #[test]
    fn detects_target_specific_dependency() {
        let m: Value = "[target.'cfg(unix)'.dev-dependencies]\namplihack-cli = \"1\"\n"
            .parse()
            .unwrap();
        assert!(find_cli_edge(&m, "").is_some());
    }

    #[test]
    fn ignores_sibling_crates() {
        let m: Value = "[dependencies]\namplihack-utils = \"1\"\namplihack-memory = \"1\"\n"
            .parse()
            .unwrap();
        assert!(find_cli_edge(&m, "").is_none());
    }
}
