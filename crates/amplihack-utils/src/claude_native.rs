//! Platform-package selection and native-binary materialization checks for
//! `@anthropic-ai/claude-code`.
//!
//! `@anthropic-ai/claude-code` ships a placeholder at `bin/claude.exe` and
//! materializes the real ~339 MB native binary in its `postinstall`
//! (`node install.cjs`), which copies the platform binary out of an
//! `optionalDependencies` package. Two independent npm flags each defeat that:
//!
//! | Flag | Mechanism | Result |
//! |---|---|---|
//! | `--ignore-scripts` | `install.cjs` never runs | stub survives |
//! | `--omit=optional` | the platform package is never fetched, so `install.cjs` finds nothing to copy | stub survives |
//!
//! The vendor's own `install.cjs` header names the second one:
//!
//! ```text
//! // If the native package isn't present (--omit=optional), prints instructions
//! // and leaves the placeholder stub in place.
//! ```
//!
//! amplihack therefore keeps **both** protective flags on every `npm install`
//! — `run_npm_install` is untouched, and copilot's argv is byte-identical — and
//! installs the one platform package for this host **explicitly, by name**,
//! exactly as the copilot path already does for `@github/copilot-linux-x64`
//! (issue #585). This module owns the name table.
//!
//! # Security
//!
//! * Every element of [`claude_platform_packages`] is a `&'static str`. That is
//!   a control, not a style choice: no runtime-derived string can reach npm's
//!   argv. Static data beats validated data.
//! * An unknown platform returns an empty slice, which skips the step
//!   non-fatally — the same posture the copilot path already takes.
//! * musl is detected with a zero-spawn filesystem probe, matching the vendor's
//!   own choice to read `process.report` rather than shell out to `ldd`.

/// The npm package whose postinstall materializes the native binary.
pub const CLAUDE_NPM_PACKAGE: &str = "@anthropic-ai/claude-code";

/// A materialized native binary is at least this large. The real one is
/// ~339 MB; the placeholder it replaces is 500 bytes.
const MIN_NATIVE_BINARY_LEN: u64 = 1024 * 1024;

/// Files at or below this size that carry no native magic number have the
/// placeholder's shape.
///
/// 4 KiB is chosen so a non-trivial shell wrapper is over the line. The
/// placeholder amplihack has seen in the wild is 500 bytes.
pub(crate) const STUB_MAX_LEN: u64 = 4096;

/// Does this file have the shape of the placeholder stub?
///
/// A **small file that does not begin with a native executable magic number**
/// (`\x7fELF`, a Mach-O magic, or `MZ`). The test is the *absence* of a magic
/// number, not the presence of any particular text: the placeholder shipped by
/// `@anthropic-ai/claude-code` has no shebang — it is 500 bytes beginning
/// `echo "Error: claude native binary not installed." >&2` and `file` reports
/// `ASCII text`. A check written to look for `#!` would miss it.
///
/// # This is a label, never a gate
///
/// It lives here, beside [`is_materialized`] and [`MIN_NATIVE_BINARY_LEN`],
/// because it encodes what a *claude* install looks like — and it is only ever
/// consulted by [`crate::launch_target`] to **relabel a candidate whose
/// `--version` probe already failed**. It was briefly a pre-probe rejection
/// applied to every tool, and that broke `amplihack copilot`: `@github/copilot`
/// ships a legitimate 1185-byte `#!/usr/bin/env node` loader, which is small
/// and carries no native magic, so it looks exactly like this. Small is not
/// broken. The probe is the authority on health; this only chooses the words.
pub(crate) fn has_placeholder_shape(head: &[u8], len: u64) -> bool {
    len <= STUB_MAX_LEN && !has_native_executable_magic(head)
}

/// Candidate platform packages for this host, in preference order.
///
/// A slice rather than a single value so a musl/glibc misdetection can be
/// corrected by one bounded retry with the alternate. An empty slice means "no
/// known package for this platform" and the caller skips the step non-fatally.
///
/// `os` and `arch` use the same spellings as `std::env::consts` and as
/// `copilot_platform_package` in `bootstrap.rs`: `"linux"` / `"macos"` /
/// `"windows"`, `"x86_64"` / `"aarch64"`.
pub fn claude_platform_packages(os: &str, arch: &str, musl: bool) -> &'static [&'static str] {
    // Every list below is a `&'static [&'static str]` literal. That is the
    // security control: there is no code path here that can build a package
    // name at runtime, so nothing derived from the environment can reach npm's
    // argv. A `format!("...-{arch}")` refactor would fail the table test.
    //
    // The Linux lists carry BOTH libc spellings, most-likely first. The musl
    // probe is a filesystem heuristic; getting it wrong must cost one bounded
    // retry, not a failed install. `-musl` packages exist on Linux only, so the
    // macOS and Windows lists have exactly one entry each.
    const LINUX_X64_GLIBC: &[&str] = &[
        "@anthropic-ai/claude-code-linux-x64",
        "@anthropic-ai/claude-code-linux-x64-musl",
    ];
    const LINUX_X64_MUSL: &[&str] = &[
        "@anthropic-ai/claude-code-linux-x64-musl",
        "@anthropic-ai/claude-code-linux-x64",
    ];
    const LINUX_ARM64_GLIBC: &[&str] = &[
        "@anthropic-ai/claude-code-linux-arm64",
        "@anthropic-ai/claude-code-linux-arm64-musl",
    ];
    const LINUX_ARM64_MUSL: &[&str] = &[
        "@anthropic-ai/claude-code-linux-arm64-musl",
        "@anthropic-ai/claude-code-linux-arm64",
    ];
    const DARWIN_ARM64: &[&str] = &["@anthropic-ai/claude-code-darwin-arm64"];
    const DARWIN_X64: &[&str] = &["@anthropic-ai/claude-code-darwin-x64"];
    const WIN32_X64: &[&str] = &["@anthropic-ai/claude-code-win32-x64"];
    const WIN32_ARM64: &[&str] = &["@anthropic-ai/claude-code-win32-arm64"];

    match (os, arch, musl) {
        ("linux", "x86_64", false) => LINUX_X64_GLIBC,
        ("linux", "x86_64", true) => LINUX_X64_MUSL,
        ("linux", "aarch64", false) => LINUX_ARM64_GLIBC,
        ("linux", "aarch64", true) => LINUX_ARM64_MUSL,
        ("macos", "aarch64", _) => DARWIN_ARM64,
        ("macos", "x86_64", _) => DARWIN_X64,
        ("windows", "x86_64", _) => WIN32_X64,
        ("windows", "aarch64", _) => WIN32_ARM64,
        // Unknown platform: no guess. The caller skips the step non-fatally,
        // exactly as the copilot path already does.
        _ => &[],
    }
}

/// Does `head` begin with a native executable magic number?
///
/// ELF, Mach-O (both endiannesses, 32- and 64-bit, plus the universal/fat
/// header), and PE. Shared with [`has_placeholder_shape`] and [`is_materialized`] so the
/// "is this a real binary?" question has exactly one answer in this crate.
fn has_native_executable_magic(head: &[u8]) -> bool {
    const MAGICS: &[&[u8]] = &[
        b"\x7fELF",                // ELF
        &[0xfe, 0xed, 0xfa, 0xce], // Mach-O 32-bit, big endian
        &[0xce, 0xfa, 0xed, 0xfe], // Mach-O 32-bit, little endian
        &[0xfe, 0xed, 0xfa, 0xcf], // Mach-O 64-bit, big endian
        &[0xcf, 0xfa, 0xed, 0xfe], // Mach-O 64-bit, little endian
        &[0xca, 0xfe, 0xba, 0xbe], // Mach-O universal
        &[0xbe, 0xba, 0xfe, 0xca], // Mach-O universal, byte-swapped
        b"MZ",                     // PE / COFF
    ];
    MAGICS.iter().any(|magic| head.starts_with(magic))
}

/// Zero-spawn probe for a musl libc: does `/lib/ld-musl-*` or
/// `/usr/lib/ld-musl-*` exist?
///
/// Ambiguity defaults to glibc (`false`). A wrong answer only reorders the
/// candidate list, and the health gate catches a wrong download.
pub fn detect_musl() -> bool {
    // Matches the vendor's own choice in install.cjs, which reads
    // `process.report.getReport().header.glibcVersionRuntime` rather than
    // shelling out to `ldd`. This is the Rust equivalent: look for the musl
    // dynamic loader on disk, spawn nothing.
    const MUSL_LOADER_DIRS: &[&str] = &["/lib", "/usr/lib"];
    for dir in MUSL_LOADER_DIRS {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_string_lossy().starts_with("ld-musl-") {
                return true;
            }
        }
    }
    false
}

/// Did the postinstall actually materialize the native binary?
///
/// Success is verified by **outcome**, never by exit code. `install.cjs`
/// returns 0 on most of its failure paths — unsupported platform, a release
/// channel with no native binaries, and a failed `require.resolve` all exit 0;
/// only a throwing `placeBinary` sets exit code 1. This is the one place in the
/// design where validation is load-bearing rather than defense-in-depth.
pub fn is_materialized(head: &[u8], len: u64) -> bool {
    // Both halves are required. Size alone would accept a large text file; the
    // magic number alone would accept a truncated download that carries a
    // correct header and nothing behind it.
    len >= MIN_NATIVE_BINARY_LEN && has_native_executable_magic(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linux_x64_glibc() {
        assert_eq!(
            claude_platform_packages("linux", "x86_64", false).first(),
            Some(&"@anthropic-ai/claude-code-linux-x64")
        );
    }

    #[test]
    fn linux_x64_musl_prefers_the_musl_package() {
        let pkgs = claude_platform_packages("linux", "x86_64", true);
        assert_eq!(
            pkgs.first(),
            Some(&"@anthropic-ai/claude-code-linux-x64-musl")
        );
        assert!(
            pkgs.contains(&"@anthropic-ai/claude-code-linux-x64"),
            "the glibc package stays in the list as the bounded retry"
        );
    }

    #[test]
    fn linux_arm64_both_libcs() {
        assert_eq!(
            claude_platform_packages("linux", "aarch64", false).first(),
            Some(&"@anthropic-ai/claude-code-linux-arm64")
        );
        assert_eq!(
            claude_platform_packages("linux", "aarch64", true).first(),
            Some(&"@anthropic-ai/claude-code-linux-arm64-musl")
        );
    }

    #[test]
    fn macos_packages_use_the_darwin_spelling() {
        assert_eq!(
            claude_platform_packages("macos", "aarch64", false),
            ["@anthropic-ai/claude-code-darwin-arm64"]
        );
        assert_eq!(
            claude_platform_packages("macos", "x86_64", false),
            ["@anthropic-ai/claude-code-darwin-x64"]
        );
    }

    #[test]
    fn macos_ignores_the_musl_hint() {
        // `-musl` variants exist on Linux only. A musl hint on macOS must not
        // produce a package name that does not exist in the registry.
        for arch in ["aarch64", "x86_64"] {
            let pkgs = claude_platform_packages("macos", arch, true);
            assert!(
                pkgs.iter().all(|p| !p.ends_with("-musl")),
                "no -musl package exists for macos/{arch}, got {pkgs:?}"
            );
        }
    }

    #[test]
    fn windows_packages_use_the_win32_spelling() {
        assert_eq!(
            claude_platform_packages("windows", "x86_64", false),
            ["@anthropic-ai/claude-code-win32-x64"]
        );
        assert_eq!(
            claude_platform_packages("windows", "aarch64", false),
            ["@anthropic-ai/claude-code-win32-arm64"]
        );
    }

    #[test]
    fn unknown_platform_is_an_empty_slice_not_a_guess() {
        assert!(claude_platform_packages("plan9", "x86_64", false).is_empty());
        assert!(claude_platform_packages("linux", "riscv64", false).is_empty());
        assert!(claude_platform_packages("", "", false).is_empty());
    }

    #[test]
    fn sec_every_package_name_is_from_the_static_table() {
        // SEC: no runtime-derived string may reach npm's argv. Assert that
        // every name is exactly one of the eight the vendor publishes, so a
        // future `format!("...-{arch}")` refactor fails here.
        const PUBLISHED: &[&str] = &[
            "@anthropic-ai/claude-code-darwin-arm64",
            "@anthropic-ai/claude-code-darwin-x64",
            "@anthropic-ai/claude-code-linux-arm64",
            "@anthropic-ai/claude-code-linux-arm64-musl",
            "@anthropic-ai/claude-code-linux-x64",
            "@anthropic-ai/claude-code-linux-x64-musl",
            "@anthropic-ai/claude-code-win32-arm64",
            "@anthropic-ai/claude-code-win32-x64",
        ];
        for os in ["linux", "macos", "windows", "plan9"] {
            for arch in ["x86_64", "aarch64", "riscv64"] {
                for musl in [true, false] {
                    for pkg in claude_platform_packages(os, arch, musl) {
                        assert!(
                            PUBLISHED.contains(pkg),
                            "{pkg} is not a published platform package ({os}/{arch}, musl={musl})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn claude_package_name_is_exact() {
        assert_eq!(CLAUDE_NPM_PACKAGE, "@anthropic-ai/claude-code");
    }

    // ------------------------------------------------------------------
    // is_materialized — outcome verification
    // ------------------------------------------------------------------

    #[test]
    fn materialized_accepts_a_real_native_binary() {
        // 338,860,336 bytes of ELF, measured on the dev VM after a correct
        // three-step install.
        assert!(is_materialized(b"\x7fELF\x02\x01\x01\x00", 338_860_336));
    }

    #[test]
    fn materialized_rejects_the_500_byte_placeholder() {
        let stub = b"echo \"Error: claude native binary not installed.\" >&2\n";
        assert!(
            !is_materialized(stub, 500),
            "the placeholder is what a 'successful' install.cjs leaves behind"
        );
    }

    #[test]
    fn materialized_rejects_a_native_magic_that_is_too_small() {
        // A truncated download carries the right magic and the wrong size.
        assert!(!is_materialized(b"\x7fELF\x02\x01\x01\x00", 4096));
    }

    #[test]
    fn materialized_rejects_a_large_file_without_native_magic() {
        // Size alone is not evidence.
        assert!(!is_materialized(b"#!/usr/bin/env node\n", 50_000_000));
    }

    #[test]
    fn materialized_accepts_mach_o_and_pe() {
        assert!(is_materialized(&[0xcf, 0xfa, 0xed, 0xfe], 90_000_000));
        assert!(is_materialized(b"MZ\x90\x00", 120_000_000));
    }

    // ------------------------------------------------------------------
    // has_placeholder_shape — the diagnosis, not a gate
    // ------------------------------------------------------------------

    /// The exact bytes of the placeholder, verified on the dev VM 2026-08-21:
    /// 500 bytes, ASCII, **no shebang**.
    fn real_stub_bytes() -> Vec<u8> {
        let mut v = b"echo \"Error: claude native binary not installed.\" >&2\nexit 1\n".to_vec();
        v.resize(500, b' ');
        v
    }

    #[test]
    fn placeholder_shape_matches_the_real_500_byte_stub() {
        let stub = real_stub_bytes();
        assert!(has_placeholder_shape(&stub, stub.len() as u64));
    }

    #[test]
    fn placeholder_shape_does_not_look_for_a_shebang() {
        // The real stub has no `#!`. A check written as "starts with #!" would
        // miss it entirely, so the test must be the ABSENCE of native magic.
        let stub = real_stub_bytes();
        assert!(
            !stub.starts_with(b"#!"),
            "fixture invariant: the real stub carries no shebang"
        );
        assert!(has_placeholder_shape(&stub, stub.len() as u64));
    }

    #[test]
    fn placeholder_shape_excludes_native_binaries() {
        assert!(!has_placeholder_shape(
            b"\x7fELF\x02\x01\x01\x00",
            338_860_336
        ));
        assert!(!has_placeholder_shape(
            &[0xcf, 0xfa, 0xed, 0xfe],
            90_000_000
        ));
        assert!(!has_placeholder_shape(b"MZ\x90\x00", 120_000_000));
    }

    #[test]
    fn placeholder_shape_excludes_anything_over_the_ceiling() {
        let wrapper = b"#!/bin/sh\nexec node /opt/claude/cli.js \"$@\"\n";
        assert!(!has_placeholder_shape(wrapper, STUB_MAX_LEN + 1));
    }

    #[test]
    fn a_small_node_shim_has_the_placeholder_shape_which_is_why_it_may_not_gate() {
        // `~/.npm-global/bin/copilot` on the dev VM: 1185 bytes,
        // `#!/usr/bin/env node`, no native magic. It is a perfectly healthy
        // binary that this predicate cannot tell apart from the stub — which is
        // the entire reason it may only ever relabel an ALREADY-FAILED probe.
        let shim = b"#!/usr/bin/env node\nrequire('@github/copilot/npm-loader.js');\n";
        assert!(
            has_placeholder_shape(shim, 1185),
            "documenting the false positive: this predicate is not evidence of \
             a broken install, only a way to name one the probe already rejected"
        );
    }

    #[test]
    fn detect_musl_does_not_panic_on_this_host() {
        // Zero-spawn probe: whatever it returns, it must return.
        let _ = detect_musl();
    }
}
