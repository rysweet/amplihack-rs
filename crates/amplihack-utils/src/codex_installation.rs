//! Read-only Codex installation validation. Installation policy belongs to callers.
//!
//! Layout evidence is pinned to upstream rust-v0.98.0, rust-v0.147.0 and rust-v0.166.0:
//! `codex-cli/bin/codex.js`, `codex-cli/scripts/build_npm_package.py`, and
//! (0.166.0) `codex-rs/install-context/src/lib.rs`.
//! The host-only implementation is present from 0.147.0; releases 0.99–0.146
//! are inconclusive because experimental host/fallback behavior varied.
use crate::launch_target::display_untrusted_path;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// A successful inspection retains the wrapper's environment and argv semantics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCodex {
    pub launch_path: PathBuf,
    pub native_path: PathBuf,
}

/// A package defect can be repaired by replacement; uncertain evidence cannot.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{detail}")]
pub struct InstallationError {
    pub detail: String,
    pub repairable: bool,
}

fn failure(path: &Path, detail: &str, repairable: bool) -> InstallationError {
    let guidance = if repairable {
        "Repair with `npm install -g @openai/codex`, or select a complete supported installation"
    } else {
        "Select a complete supported installation or address this inspection failure; automatic repair is not warranted"
    };
    InstallationError {
        detail: format!(
            "Codex {}: {detail}. {guidance}",
            display_untrusted_path(path)
        ),
        repairable,
    }
}

fn executable(path: &Path) -> Result<(), InstallationError> {
    let metadata = fs::metadata(path).map_err(|e| {
        failure(
            path,
            &format!("cannot inspect executable: {e}"),
            e.kind() == std::io::ErrorKind::NotFound,
        )
    })?;
    if !metadata.is_file() {
        return Err(failure(
            path,
            "required executable is not a regular file",
            true,
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(failure(path, "required file is not executable", true));
        }
    }
    Ok(())
}

#[derive(Deserialize)]
struct Package {
    name: String,
    version: String,
    #[serde(default)]
    bin: BTreeMap<String, String>,
    #[serde(default, rename = "optionalDependencies")]
    optional_dependencies: BTreeMap<String, String>,
}

fn package(path: &Path) -> Result<Package, InstallationError> {
    let metadata = fs::metadata(path).map_err(|e| {
        failure(
            path,
            &format!("cannot inspect package metadata: {e}"),
            e.kind() == std::io::ErrorKind::NotFound,
        )
    })?;
    if !metadata.is_file() {
        return Err(failure(
            path,
            "package metadata is not a regular file",
            true,
        ));
    }
    let file = fs::File::open(path).map_err(|e| {
        failure(
            path,
            &format!("cannot read package metadata: {e}"),
            e.kind() == std::io::ErrorKind::NotFound,
        )
    })?;
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| failure(path, &format!("cannot read package metadata: {e}"), false))?;
    if bytes.len() > 65536 {
        return Err(failure(path, "package metadata exceeds 64 KiB", false));
    }
    serde_json::from_slice(&bytes).map_err(|_| failure(path, "invalid package metadata", true))
}

fn platform() -> Option<(String, String)> {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        _ => return None,
    };
    let (os, suffix) = match std::env::consts::OS {
        "linux" => ("linux", "unknown-linux-musl"),
        "macos" => ("darwin", "apple-darwin"),
        "windows" => ("win32", "pc-windows-msvc"),
        _ => return None,
    };
    let rust_arch = if arch == "x64" { "x86_64" } else { "aarch64" };
    Some((
        format!("{rust_arch}-{suffix}"),
        format!("codex-{os}-{arch}"),
    ))
}

fn native_path(resolved: &Path, modern: bool, version: &str) -> Result<PathBuf, InstallationError> {
    if resolved.file_name().is_none_or(|n| n != "codex.js") {
        if resolved
            .extension()
            .is_some_and(|e| e == "js" || e == "cmd" || e == "ps1")
        {
            return Err(failure(
                resolved,
                "unrecognized wrapper layout; usability is inconclusive",
                false,
            ));
        }
        return Ok(resolved.to_path_buf());
    }
    let root = resolved
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| failure(resolved, "wrapper has no package root", false))?;
    if !modern && version != "0.98.0" {
        return Err(failure(
            resolved,
            "unverified legacy npm wrapper layout; usability is inconclusive",
            false,
        ));
    }
    let metadata = package(&root.join("package.json"))?;
    if metadata.name != "@openai/codex"
        || metadata.version != version
        || metadata
            .bin
            .get("codex")
            .is_none_or(|p| p != "bin/codex.js" && p != "./bin/codex.js")
    {
        return Err(failure(
            resolved,
            "unsupported official wrapper metadata; usability is inconclusive",
            false,
        ));
    }
    let (target, platform_package) = platform().ok_or_else(|| {
        failure(
            resolved,
            "unsupported npm platform; usability is inconclusive",
            false,
        )
    })?;
    let platform_version = format!(
        "{version}-{}",
        platform_package.trim_start_matches("codex-")
    );
    if modern
        && metadata
            .optional_dependencies
            .get(&format!("@openai/{platform_package}"))
            != Some(&format!("npm:@openai/codex@{platform_version}"))
    {
        return Err(failure(
            resolved,
            "unsupported platform dependency selection; usability is inconclusive",
            false,
        ));
    }
    let mut vendor = root.join("vendor");
    if modern {
        // Match Node's ancestor node_modules search, including nested dependencies.
        // Once a package is selected, a broken native tree must not fall back.
        for ancestor in resolved.parent().expect("wrapper has a parent").ancestors() {
            if ancestor
                .file_name()
                .is_some_and(|name| name == "node_modules")
            {
                continue;
            }
            let candidate = ancestor
                .join("node_modules/@openai")
                .join(&platform_package);
            match fs::metadata(candidate.join("package.json")) {
                Ok(_) => {
                    let metadata = package(&candidate.join("package.json"))?;
                    if metadata.name != "@openai/codex" || metadata.version != platform_version {
                        return Err(failure(
                            &candidate,
                            "incorrect platform package identity",
                            true,
                        ));
                    }
                    vendor = candidate.join("vendor");
                    break;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => {
                    return Err(failure(
                        &candidate,
                        &format!("cannot inspect platform package: {e}"),
                        false,
                    ));
                }
            }
        }
    }
    let name = if cfg!(windows) { "codex.exe" } else { "codex" };
    Ok(vendor
        .join(target)
        .join(if modern { "bin" } else { "codex" })
        .join(name))
}

/// Prove native executable and version-specific helper availability without
/// starting a session, reading configuration, or contacting a service.
/// Unknown versions fail closed; help/version output cannot prove capability.
pub fn validate_codex_installation(
    candidate: &Path,
    version: &str,
) -> Result<ValidatedCodex, InstallationError> {
    if !candidate.is_absolute() {
        return Err(failure(
            candidate,
            "executable path must be absolute",
            false,
        ));
    }
    executable(candidate)?;
    let resolved = fs::canonicalize(candidate)
        .map_err(|e| failure(candidate, &format!("cannot resolve executable: {e}"), false))?;
    let parsed = semver::Version::parse(version).map_err(|_| {
        failure(
            candidate,
            "unknown version; usability is inconclusive",
            false,
        )
    })?;
    // Upstream 97576b179487 removes the in-process fallback; 0.146.0
    // predates that change, and 0.147.0 includes it. Pin the upper boundary
    // too: future versions may add helpers or change package selection.
    let legacy = parsed.major == 0 && parsed.minor <= 98 && parsed.pre.is_empty();
    let modern = parsed >= semver::Version::new(0, 147, 0)
        && parsed <= semver::Version::new(0, 166, 0)
        && parsed.pre.is_empty();
    if !legacy && !modern {
        return Err(failure(
            candidate,
            "unverified helper requirements for this version; usability is inconclusive",
            false,
        ));
    }
    let native = native_path(&resolved, modern, version)?;
    executable(&native)?;
    let native = fs::canonicalize(&native).map_err(|e| {
        failure(
            &native,
            &format!("cannot resolve native executable: {e}"),
            false,
        )
    })?;
    if modern {
        let name = if cfg!(windows) {
            "codex-code-mode-host.exe"
        } else {
            "codex-code-mode-host"
        };
        let bin = native.parent().expect("absolute executable has a parent");
        // Upstream prefers packaged resources when the package marker exists.
        let root = bin
            .parent()
            .filter(|_| bin.file_name().is_some_and(|n| n == "bin"));
        let resource = root.map(|r| r.join("codex-resources").join(name));
        let helper = match resource {
            Some(path)
                if root.is_some_and(|r| r.join("codex-package.json").is_file())
                    && path.is_file() =>
            {
                path
            }
            _ => bin.join(name),
        };
        executable(&helper).map_err(|e| {
            failure(
                candidate,
                &format!(
                    "resolved native {} requires helper: {}",
                    display_untrusted_path(&native),
                    e.detail
                ),
                e.repairable,
            )
        })?;
    }
    Ok(ValidatedCodex {
        launch_path: candidate.to_path_buf(),
        native_path: native,
    })
}
