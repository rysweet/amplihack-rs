//! TDD (Step 7) — RED-first structural + security specification for the
//! `bundle_generator` decomposition refactor (issue #846).
//!
//! These tests define the *contract* for the refactor. They are expected to
//! **FAIL** against the current monolithic `src/bundle_generator.rs` and to
//! **PASS** once the file has been decomposed into the
//! `src/bundle_generator/` directory module described in the design spec:
//!
//! ```text
//! bundle_generator/
//!   mod.rs          re-exports (public API preservation)
//!   error.rs        BundleGeneratorError + recovery_suggestion
//!   models.rs       serde DTOs (ParsedPrompt .. GenerationMetrics)
//!   traits.rs       PromptParser, IntentExtractor, AgentGenerator, BundleBuilder
//!   packager.rs     FilesystemPackager, validate_output_dir, UNSAFE_PATHS
//!   distributor.rs  GitHubDistributor, truncate_to_char_boundary
//! ```
//!
//! They inspect the crate source tree (via `CARGO_MANIFEST_DIR`) because the
//! deliverable of this refactor *is* the physical module layout and the
//! placement of security-sensitive code. Behavior preservation is covered
//! separately in `bundle_generator_api_surface.rs`.

use std::fs;
use std::path::{Path, PathBuf};

/// Absolute path to `crates/amplihack-utils/src`.
fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Absolute path to the expected `bundle_generator/` module directory.
fn module_dir() -> PathBuf {
    src_dir().join("bundle_generator")
}

/// Read a submodule source file (e.g. `"distributor.rs"`), or `String::new()`
/// if it does not yet exist. Never panics so assertions produce clear
/// red-phase failures rather than I/O panics.
fn read_submodule(name: &str) -> String {
    fs::read_to_string(module_dir().join(name)).unwrap_or_default()
}

/// Concatenate every `*.rs` file inside the module directory. Returns an
/// empty string if the directory does not exist yet.
fn read_all_submodules() -> String {
    let dir = module_dir();
    let mut combined = String::new();
    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            if let Ok(text) = fs::read_to_string(&path) {
                combined.push_str(&text);
                combined.push('\n');
            }
        }
    }
    combined
}

/// Strip ASCII whitespace so content assertions are insensitive to rustfmt
/// spacing decisions during the move.
fn without_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

// ---------------------------------------------------------------------------
// Physical layout — the monolith becomes a directory module
// ---------------------------------------------------------------------------

#[test]
fn module_directory_exists() {
    assert!(
        module_dir().is_dir(),
        "expected directory module at {}",
        module_dir().display()
    );
}

#[test]
fn monolithic_single_file_is_removed() {
    let monolith = src_dir().join("bundle_generator.rs");
    assert!(
        !monolith.exists(),
        "the 1399-line monolith {} must be replaced by the \
         bundle_generator/ directory module",
        monolith.display()
    );
}

#[test]
fn all_expected_submodules_exist() {
    for file in [
        "mod.rs",
        "error.rs",
        "models.rs",
        "traits.rs",
        "packager.rs",
        "distributor.rs",
    ] {
        let path = module_dir().join(file);
        assert!(
            path.is_file(),
            "expected submodule file {} to exist",
            path.display()
        );
    }
}

#[test]
fn no_module_inception() {
    // There must be no submodule named `bundle_generator` inside the
    // directory (would trigger clippy::module_inception).
    let nested = module_dir().join("bundle_generator.rs");
    assert!(
        !nested.exists(),
        "bundle_generator/bundle_generator.rs would cause module_inception"
    );
    assert!(
        !read_all_submodules().contains("mod bundle_generator"),
        "no submodule may be named bundle_generator"
    );
}

// ---------------------------------------------------------------------------
// Content placement — each item lands in its designated submodule
// ---------------------------------------------------------------------------

#[test]
fn error_type_lives_in_error_module() {
    assert!(
        read_submodule("error.rs").contains("enum BundleGeneratorError"),
        "BundleGeneratorError must be defined in error.rs"
    );
}

#[test]
fn data_models_live_in_models_module() {
    let models = read_submodule("models.rs");
    for ty in [
        "struct ParsedPrompt",
        "struct AgentRequirement",
        "struct ExtractedIntent",
        "struct GeneratedAgent",
        "struct AgentBundle",
        "struct PackagedBundle",
        "struct DistributionResult",
        "struct TestResult",
        "struct GenerationMetrics",
    ] {
        assert!(
            models.contains(ty),
            "expected `{ty}` to be defined in models.rs"
        );
    }
}

#[test]
fn pipeline_traits_live_in_traits_module() {
    let traits = read_submodule("traits.rs");
    for tr in [
        "trait PromptParser",
        "trait IntentExtractor",
        "trait AgentGenerator",
        "trait BundleBuilder",
    ] {
        assert!(
            traits.contains(tr),
            "expected `{tr}` to be defined in traits.rs"
        );
    }
}

#[test]
fn packager_lives_in_packager_module() {
    let packager = read_submodule("packager.rs");
    assert!(
        packager.contains("struct FilesystemPackager"),
        "FilesystemPackager must be defined in packager.rs"
    );
    assert!(
        packager.contains("fn validate_output_dir"),
        "validate_output_dir path-safety guard must live in packager.rs"
    );
    assert!(
        packager.contains("UNSAFE_PATHS"),
        "UNSAFE_PATHS blocklist must live in packager.rs"
    );
}

#[test]
fn distributor_lives_in_distributor_module() {
    let distributor = read_submodule("distributor.rs");
    assert!(
        distributor.contains("struct GitHubDistributor"),
        "GitHubDistributor must be defined in distributor.rs"
    );
    assert!(
        distributor.contains("fn truncate_to_char_boundary"),
        "truncate_to_char_boundary must live in distributor.rs"
    );
}

// ---------------------------------------------------------------------------
// Public API preservation — mod.rs must re-export the surface
// ---------------------------------------------------------------------------

#[test]
fn mod_reexports_public_surface() {
    let mod_rs = read_submodule("mod.rs");
    assert!(
        mod_rs.contains("pub use"),
        "mod.rs must re-export submodule items to preserve the \
         bundle_generator::* public paths"
    );
    // Key public items must be reachable via re-export so external
    // `amplihack_utils::bundle_generator::X` paths remain byte-compatible.
    for item in [
        "BundleGeneratorError",
        "GitHubDistributor",
        "FilesystemPackager",
        "AgentBundle",
        "PromptParser",
    ] {
        assert!(
            mod_rs.contains(item),
            "mod.rs must re-export `{item}` to preserve the public API"
        );
    }
}

// ---------------------------------------------------------------------------
// Security invariants — must survive the move verbatim
// ---------------------------------------------------------------------------

#[test]
fn github_token_field_stays_private() {
    // The auth token must never be widened to pub / pub(crate).
    let normalized = without_ws(&read_all_submodules());
    assert!(
        !normalized.contains("pubtoken:String") && !normalized.contains("pub(crate)token:String"),
        "GitHubDistributor.token MUST remain a private field"
    );
}

#[test]
fn github_token_only_lives_in_distributor_module() {
    // `self.token` (the auth secret) must not leak into any other submodule.
    for file in ["error.rs", "models.rs", "traits.rs", "packager.rs"] {
        assert!(
            !read_submodule(file).contains("self.token"),
            "auth token handling must stay confined to distributor.rs, \
             found `self.token` in {file}"
        );
    }
}

#[test]
fn token_is_used_only_for_env_injection() {
    // Every use of the secret must be a `gh` env injection — never
    // interpolated into an argv, a log line, or an error message.
    let distributor = without_ws(&read_submodule("distributor.rs"));
    let uses = distributor.matches("self.token").count();
    let env_injections = distributor
        .matches(".env(\"GH_TOKEN\",&self.token)")
        .count();
    assert!(
        uses > 0,
        "distributor.rs should reference self.token for env injection"
    );
    assert_eq!(
        uses, env_injections,
        "every `self.token` use must be a `.env(\"GH_TOKEN\", &self.token)` \
         injection ({uses} uses vs {env_injections} env injections) — the \
         token must never reach argv, logs, or error strings"
    );
}

#[test]
fn gh_is_invoked_without_a_shell() {
    // Subprocess exec must use argv form, never a shell string (no
    // shell-injection vector).
    let distributor = read_submodule("distributor.rs");
    if !distributor.is_empty() {
        assert!(
            distributor.contains("Command::new(\"gh\")"),
            "distributor.rs must invoke gh via Command::new(\"gh\")"
        );
    }
    let all = read_all_submodules();
    assert!(
        !all.contains("Command::new(\"sh\")") && !all.contains("sh -c"),
        "gh must never be invoked through a shell (no `sh -c`)"
    );
}
