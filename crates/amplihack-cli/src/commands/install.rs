//! Native install and uninstall commands.

use crate::command_error::exit_error;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::BTreeSet;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

const REPO_URL: &str = "https://github.com/rysweet/amplihack";
const ESSENTIAL_DIRS: &[&str] = &[
    "agents/amplihack",
    "commands/amplihack",
    "tools/amplihack",
    "tools/xpia",
    "context",
    "workflow",
    "skills",
    "templates",
    "scenarios",
    "docs",
    "schemas",
    "config",
];
const ESSENTIAL_FILES: &[&str] = &["tools/statusline.sh", "AMPLIHACK.md"];
const RUNTIME_DIRS: &[&str] = &[
    "runtime",
    "runtime/logs",
    "runtime/metrics",
    "runtime/security",
    "runtime/analysis",
];
const AMPLIHACK_HOOK_FILES: &[&str] = &[
    "session_start.py",
    "stop.py",
    "pre_tool_use.py",
    "post_tool_use.py",
    "user_prompt_submit.py",
    "workflow_classification_reminder.py",
    "pre_compact.py",
];
const XPIA_HOOK_FILES: &[&str] = &["session_start.py", "post_tool_use.py", "pre_tool_use.py"];

/// Discriminates between hook command styles.
#[derive(Clone)]
enum HookCommandKind {
    /// Invokes the amplihack-hooks binary with a specific subcommand.
    BinarySubcmd { subcmd: &'static str },
    /// Invokes a Python file directly by absolute path.
    PythonFile { file: &'static str },
}

#[derive(Clone)]
struct HookSpec {
    event: &'static str,
    cmd: HookCommandKind,
    timeout: Option<u64>,
    matcher: Option<&'static str>,
}

const AMPLIHACK_HOOK_SPECS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "Stop",
        cmd: HookCommandKind::BinarySubcmd { subcmd: "stop" },
        timeout: Some(120),
        matcher: None,
    },
    HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    HookSpec {
        event: "PostToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "post-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    // PythonFile entry must come BEFORE the BinarySubcmd for UserPromptSubmit
    // so Claude Code executes the classification reminder (5 s budget) first.
    HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::PythonFile {
            file: "workflow_classification_reminder.py",
        },
        timeout: Some(5),
        matcher: None,
    },
    HookSpec {
        event: "UserPromptSubmit",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "user-prompt-submit",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "PreCompact",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-compact",
        },
        timeout: Some(30),
        matcher: None,
    },
];

const XPIA_HOOK_SPECS: &[HookSpec] = &[
    HookSpec {
        event: "SessionStart",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        },
        timeout: Some(10),
        matcher: None,
    },
    HookSpec {
        event: "PreToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "pre-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
    HookSpec {
        event: "PostToolUse",
        cmd: HookCommandKind::BinarySubcmd {
            subcmd: "post-tool-use",
        },
        timeout: None,
        matcher: Some("*"),
    },
];

#[derive(Debug, Default, Serialize, Deserialize)]
struct InstallManifest {
    files: Vec<String>,
    dirs: Vec<String>,
    #[serde(default)]
    binaries: Vec<String>,
    #[serde(default)]
    hook_registrations: Vec<String>,
}

/// Resolve the amplihack-hooks binary through a 5-step chain.
///
/// 1. `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` env var (if set AND the path exists)
/// 2. Sibling of the current executable
/// 3. PATH lookup (handles reinstall after uninstall removes ~/.local/bin copy)
/// 4. `~/.local/bin/amplihack-hooks`
/// 5. `~/.cargo/bin/amplihack-hooks`
fn find_hooks_binary() -> Result<PathBuf> {
    // Step 1: env var override
    if let Some(val) = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") {
        let p = PathBuf::from(&val);
        if p.exists() {
            return Ok(p);
        }
        bail!(
            "AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH is set to {:?} but that path does not exist",
            p
        );
    }

    // Step 2: sibling of current exe
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("amplihack-hooks");
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }

    // Step 3: PATH lookup — finds system-wide installs (e.g. /usr/local/bin) even
    // after uninstall has removed the ~/.local/bin copy.
    if let Some(found) = find_binary("amplihack-hooks") {
        return Ok(found);
    }

    // Steps 4-5: ~/.local/bin then ~/.cargo/bin — one home_dir() call covers both.
    if let Ok(home) = home_dir() {
        for suffix in &[".local/bin", ".cargo/bin"] {
            let candidate = home.join(suffix).join("amplihack-hooks");
            if is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }

    bail!(
        "amplihack-hooks binary not found. Install it with:\n  \
         cargo install --git https://github.com/rysweet/amplihack amplihack-hooks\n  \
         or set AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH to its location."
    )
}

/// Validate that a binary path contains no shell metacharacters that could cause
/// shell misinterpretation or bypass command-string validation.
///
/// Blocks: space (` `), single quote (`'`), double quote (`"`), backslash (`\`),
/// plus all the shell operator characters blocked by `validate_hook_command_string`.
fn validate_binary_path(path: &str) -> Result<()> {
    const BLOCKED: &[char] = &[
        '|', '&', ';', '$', '`', '(', ')', '{', '}', '<', '!', '>', '#', '~', '*', ' ', '\'', '"',
        '\\',
    ];
    for ch in BLOCKED {
        if path.contains(*ch) {
            bail!("binary path contains unsafe character '{ch}': {path}");
        }
    }
    Ok(())
}

/// Validate that a hook command string contains no shell metacharacters.
///
/// Blocks: `|`, `&`, `;`, `$`, backtick, `(`, `)`, `{`, `}`, `<`, `!`, `>`, `#`, `~`, `*`
fn validate_hook_command_string(cmd: &str) -> Result<()> {
    const BLOCKED: &[char] = &[
        '|', '&', ';', '$', '`', '(', ')', '{', '}', '<', '!', '>', '#', '~', '*',
    ];
    for ch in BLOCKED {
        if cmd.contains(*ch) {
            bail!("hook command string contains unsafe metacharacter '{ch}': {cmd}");
        }
    }
    Ok(())
}

/// Copy the amplihack-hooks binary (and self) to `~/.local/bin` with 0o755 perms.
/// Emits a PATH advisory if `~/.local/bin` is not in `$PATH`.
/// Returns the list of deployed paths for the manifest.
fn deploy_binaries() -> Result<Vec<PathBuf>> {
    let home = home_dir()?;
    let local_bin = home.join(".local").join("bin");
    fs::create_dir_all(&local_bin)
        .with_context(|| format!("failed to create {}", local_bin.display()))?;

    let hooks_src = find_hooks_binary()?;
    let hooks_dst = local_bin.join("amplihack-hooks");

    fs::copy(&hooks_src, &hooks_dst).with_context(|| {
        format!(
            "failed to copy {} to {}",
            hooks_src.display(),
            hooks_dst.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hooks_dst, std::fs::Permissions::from_mode(0o755))
            .with_context(|| format!("failed to chmod {}", hooks_dst.display()))?;
    }

    let mut deployed = vec![hooks_dst.clone()];

    // Also copy self (the amplihack binary) if it differs from the destination
    if let Ok(self_exe) = std::env::current_exe() {
        let self_dst = local_bin.join("amplihack");
        if self_exe != self_dst {
            fs::copy(&self_exe, &self_dst).with_context(|| {
                format!("failed to copy amplihack binary to {}", self_dst.display())
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(e) =
                    fs::set_permissions(&self_dst, std::fs::Permissions::from_mode(0o755))
                {
                    tracing::warn!(
                        "failed to set executable bit on {}: {}",
                        self_dst.display(),
                        e
                    );
                }
            }
            deployed.push(self_dst);
        }
    }

    // PATH advisory
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let in_path = std::env::split_paths(&path_var).any(|dir| dir == local_bin);
    if !in_path {
        println!(
            "  ⚠️  ~/.local/bin is not in $PATH. Add it to your shell profile:\n   \
             export PATH=\"$HOME/.local/bin:$PATH\""
        );
    }

    if let Some(amplihack_dst) = deployed
        .iter()
        .find(|path| path.file_name().and_then(|value| value.to_str()) == Some("amplihack"))
        && let Some(on_path) = find_binary("amplihack")
        && on_path != *amplihack_dst
    {
        println!(
            "  ⚠️  `amplihack` currently resolves to {} instead of the Rust binary at {}.",
            on_path.display(),
            amplihack_dst.display()
        );
        println!(
            "     Use {} directly for the Rust CLI, or move ~/.local/bin ahead of older amplihack installs.",
            amplihack_dst.display()
        );
    }

    Ok(deployed)
}

/// Validate that python3 is available and `import amplihack` succeeds.
///
/// SAFETY: All subprocess invocations use discrete arg arrays with hardcoded
/// constants only — no shell=true equivalent, no user-supplied strings in argv.
fn validate_python() -> Result<()> {
    // Step 1: python3 --version
    let version_status = Command::new("python3").arg("--version").status();

    match version_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            bail!(
                "python3 exited with non-zero status {}: Python 3 is required",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            bail!(
                "python3 not found in PATH: {e}\n\
                 Python 3 is required for amplihack hooks."
            );
        }
    }

    // Step 2: python3 -c 'import amplihack'
    // SAFETY: args are hardcoded constants, no user input included.
    let import_status = Command::new("python3")
        .arg("-c")
        .arg("import amplihack")
        .status();

    match import_status {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => {
            bail!(
                "python3 -c 'import amplihack' failed (exit {}): \
                 amplihack Python package not installed.\n\
                 Install with: pip install amplihack",
                s.code().unwrap_or(-1)
            );
        }
        Err(e) => {
            bail!("failed to run python3 import check: {e}");
        }
    }
}

pub fn run_install(local: Option<PathBuf>) -> Result<()> {
    if let Some(local_path) = local {
        // Validate and canonicalize the --local path
        let canonical = local_path.canonicalize().with_context(|| {
            format!(
                "--local path does not exist or cannot be canonicalized: {}",
                local_path.display()
            )
        })?;
        if !canonical.is_dir() {
            bail!("--local path is not a directory: {}", canonical.display());
        }
        return local_install(&canonical);
    }

    let temp_dir = tempfile::tempdir().context("failed to create temp dir for install")?;
    let status = Command::new("git")
        .arg("clone")
        .arg("--depth")
        .arg("1")
        .arg(REPO_URL)
        .arg(temp_dir.path())
        .status()
        .context("failed to run git clone")?;

    if !status.success() {
        let exit_status = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        println!(
            "Failed to install: Command '['git', 'clone', '--depth', '1', '{}', '{}']' returned non-zero exit status {}.",
            REPO_URL,
            temp_dir.path().display(),
            exit_status,
        );
        return Err(exit_error(1));
    }

    local_install(temp_dir.path())
}

pub(crate) fn ensure_framework_installed() -> Result<()> {
    let staging_dir = staging_claude_dir()?;
    let needs_bootstrap =
        !staging_dir.exists() || !missing_hook_paths("amplihack", AMPLIHACK_HOOK_FILES)?.is_empty();
    if needs_bootstrap {
        println!("🔧 Bootstrapping amplihack framework assets...");
        run_install(None)?;
        return Ok(());
    }

    let hooks_bin = find_hooks_binary()?;
    let timestamp = unix_timestamp();
    let (settings_ok, _events) = ensure_settings_json(&staging_dir, timestamp, &hooks_bin)?;
    if !settings_ok {
        bail!("failed to configure ~/.claude/settings.json for staged amplihack hooks");
    }
    Ok(())
}

pub fn run_uninstall() -> Result<()> {
    let claude_dir = staging_claude_dir()?;
    let manifest_path = manifest_path()?;
    let manifest = read_manifest(&manifest_path)?;

    let mut removed_any = false;
    let mut removed_files = 0usize;

    // Phase 1: remove files tracked in manifest
    for file in &manifest.files {
        let target = claude_dir.join(file);
        if target.is_file() {
            match fs::remove_file(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_files += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove file {file}: {error}");
                }
            }
        }
    }

    // Phase 2: remove dirs tracked in manifest (deepest-first to avoid removing a parent
    // before its children, which would cause remove_dir_all to fail on the children).
    let mut dirs_sorted = manifest.dirs.clone();
    dirs_sorted.sort_unstable(); // NOTE: dedup() only removes adjacent duplicates — sort must precede it
    dirs_sorted.dedup();
    for dir in dirs_sorted.iter().rev() {
        let target = claude_dir.join(dir);
        if target.is_dir() && fs::remove_dir_all(&target).is_ok() {
            removed_any = true;
        }
    }

    let mut removed_dirs = 0usize;
    for dir in ["agents/amplihack", "commands/amplihack", "tools/amplihack"] {
        let target = claude_dir.join(dir);
        if target.exists() {
            match fs::remove_dir_all(&target) {
                Ok(()) => {
                    removed_any = true;
                    removed_dirs += 1;
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove {}: {}", target.display(), error);
                }
            }
        }
    }

    // Phase 3: remove binaries listed in manifest
    for binary_path in &manifest.binaries {
        let p = PathBuf::from(binary_path);
        if p.is_file() {
            match fs::remove_file(&p) {
                Ok(()) => {
                    removed_any = true;
                    println!("  🗑️  Removed binary {}", p.display());
                }
                Err(error) => {
                    println!("  ⚠️  Could not remove binary {}: {error}", p.display());
                }
            }
        }
    }

    // Phase 4: remove hook registrations from ~/.claude/settings.json
    let global_settings = global_settings_path()?;
    if global_settings.exists() && !manifest.hook_registrations.is_empty() {
        if let Err(e) = remove_hook_registrations(&global_settings) {
            println!("  ⚠️  Could not clean hook registrations: {e}");
        } else {
            println!("  ✅ Hook registrations removed from settings.json");
        }
    }

    let _ = fs::remove_file(&manifest_path);

    if removed_any {
        println!("✅ Uninstalled amplihack from {}", claude_dir.display());
        if removed_files > 0 {
            println!("   • Removed {removed_files} files");
        }
        if removed_dirs > 0 {
            println!("   • Removed {removed_dirs} amplihack directories");
        }
    } else {
        println!("Nothing to uninstall.");
    }

    Ok(())
}

/// Remove amplihack hook registrations from settings.json.
/// Removes wrappers whose command contains `amplihack-hooks` or `tools/amplihack/`.
/// Preserves XPIA and all other non-amplihack entries.
fn remove_hook_registrations(settings_path: &Path) -> Result<()> {
    let mut settings = read_settings_json(settings_path)?;
    let root = ensure_object(&mut settings);
    if let Some(hooks_val) = root.get_mut("hooks")
        && let Some(hooks_map) = hooks_val.as_object_mut()
    {
        for (_event, wrappers_val) in hooks_map.iter_mut() {
            if let Some(wrappers) = wrappers_val.as_array_mut() {
                wrappers.retain(|wrapper| {
                    // Keep wrapper if none of its hooks reference amplihack
                    let hooks = wrapper.get("hooks").and_then(Value::as_array);
                    let Some(hooks) = hooks else {
                        return true;
                    };
                    let is_amplihack = hooks.iter().any(|hook| {
                        hook.get("command")
                            .and_then(Value::as_str)
                            .map(|cmd| {
                                cmd.contains("amplihack-hooks") || cmd.contains("tools/amplihack/")
                            })
                            .unwrap_or(false)
                    });
                    !is_amplihack
                });
            }
        }
        // Phase 2: prune event-type keys where every amplihack wrapper was removed,
        // leaving no empty arrays in settings.json (fixes issue #38).
        // Non-array values (unlikely but possible) are kept via the unwrap_or(true) guard.
        hooks_map.retain(|_event, wrappers_val| {
            wrappers_val
                .as_array()
                .map(|a| !a.is_empty())
                .unwrap_or(true)
        });
    }

    fs::write(
        settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )
    .with_context(|| format!("failed to write {}", settings_path.display()))
}

fn local_install(repo_root: &Path) -> Result<()> {
    let claude_dir = staging_claude_dir()?;
    let timestamp = unix_timestamp();

    println!();
    println!("🚀 Starting amplihack installation...");
    println!("   Source: {}", repo_root.display());
    println!("   Target: {}", claude_dir.display());
    println!();
    println!(
        "ℹ️  Profile management unavailable (No module named 'profile_management'), using full installation"
    );
    println!();

    // Phase 0: validate python3 first (fail-fast)
    println!("🐍 Validating Python environment:");
    if let Err(e) = validate_python() {
        println!("  ❌ {e}");
        return Err(exit_error(1));
    }
    println!("  ✅ python3 and amplihack module available");

    // Phase 1: deploy binaries
    println!();
    println!("🦀 Deploying binaries:");
    let deployed_binaries = deploy_binaries()?;
    let hooks_bin = find_hooks_binary()?;
    for p in &deployed_binaries {
        println!("  ✅ Deployed {}", p.display());
    }

    ensure_dirs(&claude_dir)?;
    let pre_dirs = all_rel_dirs(&claude_dir)?;

    println!();
    println!("📁 Copying essential directories:");
    let source_claude = find_source_claude_dir(repo_root)?;
    let copied_dirs = copytree_manifest(&source_claude, &claude_dir)?;
    if copied_dirs.is_empty() {
        println!();
        println!("❌ No directories were copied. Installation may be incomplete.");
        println!("   Please check that the source repository is valid.");
        println!();
        return Ok(());
    }

    println!();
    println!("📝 Initializing PROJECT.md:");
    initialize_project_md(&claude_dir)?;

    println!();
    println!("📂 Creating runtime directories:");
    create_runtime_dirs(&claude_dir)?;

    println!();
    println!("⚙️  Configuring settings.json:");
    let (settings_ok, registered_events) =
        ensure_settings_json(&claude_dir, timestamp, &hooks_bin)?;

    println!();
    println!("🔍 Verifying hook files:");
    let hooks_ok = verify_hooks()?;

    println!();
    println!("🦀 Ensuring Rust recipe runner:");
    if find_binary("recipe-runner-rs").is_some() {
        println!("   ✅ recipe-runner-rs is available");
    } else {
        println!("   ❌ recipe-runner-rs not installed (recipe execution will fail without it)");
        println!(
            "   Install: cargo install --git https://github.com/rysweet/amplihack-recipe-runner"
        );
    }

    println!();
    println!("📝 Generating uninstall manifest:");
    let manifest_path = manifest_path()?;
    let mut tracked_roots = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        if full.exists() {
            tracked_roots.push(full);
        }
    }
    let (files, post_dirs) = get_all_files_and_dirs(&claude_dir, &tracked_roots)?;
    let new_dirs = post_dirs
        .into_iter()
        .filter(|dir| !pre_dirs.contains(dir))
        .collect::<Vec<_>>();

    let manifest = InstallManifest {
        files,
        dirs: new_dirs,
        binaries: deployed_binaries
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect(),
        hook_registrations: registered_events,
    };
    write_manifest(&manifest_path, &manifest)?;
    println!("   Manifest written to {}", manifest_path.display());

    println!();
    println!("============================================================");
    if settings_ok && hooks_ok && !copied_dirs.is_empty() {
        println!("✅ Amplihack installation completed successfully!");
        println!();
        println!("📍 Installed to: {}", claude_dir.display());
        println!();
        println!("📦 Components installed:");
        for dir in &copied_dirs {
            println!("   • {dir}");
        }
        println!();
        println!("🎯 Features enabled:");
        println!("   • Session start hook");
        println!("   • Stop hook");
        println!("   • Post-tool-use hook");
        println!("   • Pre-compact hook");
        println!("   • Runtime logging and metrics");
        println!();
        println!("💡 To uninstall: amplihack uninstall");
    } else {
        println!("⚠️  Installation completed with warnings");
        if !settings_ok {
            println!("   • Settings.json configuration had issues");
        }
        if !hooks_ok {
            println!("   • Some hook files are missing");
        }
        if copied_dirs.is_empty() {
            println!("   • No directories were copied");
        }
        println!();
        println!("💡 You may need to manually verify the installation");
    }
    println!("============================================================");
    println!();

    Ok(())
}

fn ensure_dirs(claude_dir: &Path) -> Result<()> {
    fs::create_dir_all(claude_dir)
        .with_context(|| format!("failed to create {}", claude_dir.display()))
}

fn find_source_claude_dir(repo_root: &Path) -> Result<PathBuf> {
    let direct = repo_root.join(".claude");
    if direct.exists() {
        return Ok(direct);
    }
    let parent = repo_root.join("..").join(".claude");
    if parent.exists() {
        return Ok(parent);
    }
    anyhow::bail!(
        ".claude not found at {} or {}",
        direct.display(),
        parent.display()
    )
}

fn copytree_manifest(source_claude: &Path, claude_dir: &Path) -> Result<Vec<String>> {
    let mut copied = Vec::new();
    for dir in ESSENTIAL_DIRS {
        let source_dir = source_claude.join(dir);
        if !source_dir.exists() {
            println!("  ⚠️  Warning: {dir} not found in source, skipping");
            continue;
        }

        let target_dir = claude_dir.join(dir);
        if let Some(parent) = target_dir.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }

        copy_dir_recursive(&source_dir, &target_dir)?;
        if dir.starts_with("tools/") {
            let files_updated = set_hook_permissions(&target_dir)?;
            if files_updated > 0 {
                println!("  🔐 Set execute permissions on {files_updated} hook files");
            }
        }
        println!("  ✅ Copied {dir}");
        copied.push((*dir).to_string());
    }

    let settings_src = source_claude.join("settings.json");
    let settings_dst = claude_dir.join("settings.json");
    if settings_src.exists() && !settings_dst.exists() {
        fs::copy(&settings_src, &settings_dst).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_src.display(),
                settings_dst.display()
            )
        })?;
        println!("  ✅ Copied settings.json");
    }

    for file in ESSENTIAL_FILES {
        let source_file = source_claude.join(file);
        if !source_file.exists() {
            println!("  ⚠️  Warning: {file} not found in source, skipping");
            continue;
        }
        let target_file = claude_dir.join(file);
        if let Some(parent) = target_file.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::copy(&source_file, &target_file).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_file.display(),
                target_file.display()
            )
        })?;
        set_script_permissions(&target_file)?;
        println!("  ✅ Copied {file}");
    }

    let source_claude_md = source_claude
        .parent()
        .context("source .claude dir missing parent")?
        .join("CLAUDE.md");
    if source_claude_md.exists() {
        let target_claude_md = claude_dir
            .parent()
            .context("target .claude dir missing parent")?
            .join("CLAUDE.md");
        fs::copy(&source_claude_md, &target_claude_md).with_context(|| {
            format!(
                "failed to copy {} to {}",
                source_claude_md.display(),
                target_claude_md.display()
            )
        })?;
        println!("  ✅ Installed amplihack CLAUDE.md");
    }

    Ok(copied)
}

fn initialize_project_md(claude_dir: &Path) -> Result<()> {
    let project_md = claude_dir.join("context").join("PROJECT.md");
    if let Some(parent) = project_md.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(
        &project_md,
        "# Project Overview\n\nThis PROJECT.md was initialized by amplihack.\n",
    )
    .with_context(|| format!("failed to write {}", project_md.display()))?;
    println!("   ✅ PROJECT.md initialized using template");
    Ok(())
}

fn create_runtime_dirs(claude_dir: &Path) -> Result<()> {
    for dir in RUNTIME_DIRS {
        let full = claude_dir.join(dir);
        fs::create_dir_all(&full)
            .with_context(|| format!("failed to create {}", full.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755))
                .with_context(|| format!("failed to set permissions on {}", full.display()))?;
        }
        println!("  ✅ Runtime directory {dir} ready");
    }
    Ok(())
}

/// Configure ~/.claude/settings.json with amplihack hook registrations.
///
/// Returns `(success, registered_event_names)` where `registered_event_names`
/// is a deduplicated list of event names that were configured.
fn ensure_settings_json(
    staging_dir: &Path,
    timestamp: u64,
    hooks_bin: &Path,
) -> Result<(bool, Vec<String>)> {
    let settings_path = global_settings_path()?;
    if let Some(parent) = settings_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let backup_path = settings_path
        .parent()
        .context("global settings path missing parent")?
        .join(format!("settings.json.backup.{timestamp}"));
    if settings_path.exists() {
        fs::copy(&settings_path, &backup_path).with_context(|| {
            format!(
                "failed to copy {} to {}",
                settings_path.display(),
                backup_path.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&backup_path, std::fs::Permissions::from_mode(0o600));
        }
        let backup_dir = staging_dir.join("runtime").join("sessions");
        fs::create_dir_all(&backup_dir)
            .with_context(|| format!("failed to create {}", backup_dir.display()))?;
        // Use serde_json::json! to guarantee valid JSON even with special chars in paths
        let metadata = json!({
            "settings_path": settings_path.to_string_lossy().as_ref(),
            "backup_path": backup_path.to_string_lossy().as_ref(),
        });
        fs::write(
            backup_dir.join(format!("install_{timestamp}_backup.json")),
            serde_json::to_string_pretty(&metadata)?,
        )
        .context("failed to write install backup metadata")?;
        println!("  💾 Backup created at {}", backup_path.display());
        println!("  📋 Found existing settings.json");
    } else {
        fs::write(&settings_path, "{}\n")
            .with_context(|| format!("failed to write {}", settings_path.display()))?;
    }

    let missing = missing_hook_paths("amplihack", AMPLIHACK_HOOK_FILES)?;
    if !missing.is_empty() {
        println!("  ❌ Hook validation failed - missing required hooks:");
        for missing_hook in missing {
            println!("     • {missing_hook}");
        }
        println!("  💡 Please reinstall amplihack to restore missing hooks");
        return Ok((false, Vec::new()));
    }

    let mut settings = read_settings_json(&settings_path)?;
    ensure_permissions(&mut settings);
    update_hook_paths(&mut settings, "amplihack", AMPLIHACK_HOOK_SPECS, hooks_bin);

    let xpia_dir = xpia_hooks_dir()?;
    if xpia_dir.exists() {
        update_hook_paths(&mut settings, "xpia", XPIA_HOOK_SPECS, hooks_bin);
    }

    fs::write(
        &settings_path,
        serde_json::to_string_pretty(&settings)? + "\n",
    )
    .with_context(|| format!("failed to write {}", settings_path.display()))?;
    println!("  ✅ settings.json configured");

    // Collect deduplicated event names that were registered
    let registered_events: Vec<String> = {
        let mut seen = BTreeSet::new();
        for spec in AMPLIHACK_HOOK_SPECS {
            seen.insert(spec.event.to_string());
        }
        seen.into_iter().collect()
    };

    Ok((true, registered_events))
}

fn verify_hooks() -> Result<bool> {
    let mut all_exist = true;
    println!("  📋 Amplihack hooks:");
    for file in AMPLIHACK_HOOK_FILES {
        let hook_path = amplihack_hooks_dir()?.join(file);
        if hook_path.exists() {
            println!("    ✅ {file} found");
        } else {
            println!("    ❌ {file} missing");
            all_exist = false;
        }
    }

    let xpia_dir = xpia_hooks_dir()?;
    if !xpia_dir.exists() {
        println!("  ℹ️  XPIA security hooks not installed (optional feature)");
    } else {
        for file in XPIA_HOOK_FILES {
            let hook_path = xpia_dir.join(file);
            if hook_path.exists() {
                println!("    ✅ {file} found");
            } else {
                println!("    ❌ {file} missing");
            }
        }
    }

    Ok(all_exist)
}

fn read_settings_json(settings_path: &Path) -> Result<Value> {
    let raw = fs::read_to_string(settings_path)
        .with_context(|| format!("failed to read {}", settings_path.display()))?;
    match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(map)) => Ok(Value::Object(map)),
        Ok(_) => Ok(Value::Object(Map::new())),
        Err(_) => {
            tracing::warn!(
                "Settings file {} contains invalid JSON, using empty defaults",
                settings_path.display()
            );
            Ok(Value::Object(Map::new()))
        }
    }
}

fn ensure_permissions(settings: &mut Value) {
    let root = ensure_object(settings);
    let permissions = root
        .entry("permissions")
        .or_insert_with(|| Value::Object(Map::new()));
    let permissions = ensure_object(permissions);

    permissions
        .entry("allow")
        .or_insert_with(|| json!(["Bash", "TodoWrite", "WebSearch", "WebFetch"]));
    permissions
        .entry("deny")
        .or_insert_with(|| Value::Array(Vec::new()));
    permissions
        .entry("defaultMode")
        .or_insert_with(|| Value::String("bypassPermissions".to_string()));

    let additional = permissions
        .entry("additionalDirectories")
        .or_insert_with(|| json!([".claude", "Specs"]));
    let additional = ensure_array(additional);
    for dir in [".claude", "Specs"] {
        if !additional.iter().any(|value| value.as_str() == Some(dir)) {
            additional.push(Value::String(dir.to_string()));
        }
    }
}

fn update_hook_paths(
    settings: &mut Value,
    hook_system: &str,
    specs: &[HookSpec],
    hooks_bin: &Path,
) {
    let root = ensure_object(settings);
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks = ensure_object(hooks);

    for spec in specs {
        let wrappers = hooks
            .entry(spec.event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let wrappers = ensure_array(wrappers);
        let desired = build_hook_wrapper(spec, hooks_bin);

        if let Some(existing) = wrappers
            .iter_mut()
            .find(|wrapper| wrapper_matches(wrapper, spec, hook_system))
        {
            *existing = desired;
        } else {
            wrappers.push(desired);
        }
    }
}

/// Build the Claude Code hook wrapper JSON for a given spec.
///
/// For `BinarySubcmd`: command = `"{hooks_bin} {subcmd}"`
/// For `PythonFile`: command = absolute path to the Python file in tools/amplihack/hooks/
/// Wrap a path string in double quotes, escaping any embedded double quotes.
fn shell_quote_path(path_str: &str) -> String {
    let escaped = path_str.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

fn build_hook_wrapper(spec: &HookSpec, hooks_bin: &Path) -> Value {
    let command_str = match &spec.cmd {
        HookCommandKind::BinarySubcmd { subcmd } => {
            let bin_str = hooks_bin.display().to_string();
            validate_binary_path(&bin_str)
                .expect("hooks binary path must not contain shell-unsafe characters");
            let quoted_bin = shell_quote_path(&bin_str);
            format!("{quoted_bin} {subcmd}")
        }
        HookCommandKind::PythonFile { file } => {
            // Build absolute path: ~/.amplihack/.claude/tools/amplihack/hooks/<file>
            let path_str = amplihack_hooks_dir()
                .map(|dir| dir.join(file).to_string_lossy().into_owned())
                .unwrap_or_else(|_| file.to_string());
            validate_binary_path(&path_str)
                .expect("hook Python file path must not contain shell-unsafe characters");
            shell_quote_path(&path_str)
        }
    };
    validate_hook_command_string(&command_str)
        .expect("hook command strings are built from controlled paths and literals");

    let mut hook = Map::new();
    hook.insert("type".to_string(), Value::String("command".to_string()));
    hook.insert("command".to_string(), Value::String(command_str));
    if let Some(timeout) = spec.timeout {
        hook.insert("timeout".to_string(), Value::Number(timeout.into()));
    }

    let mut wrapper = Map::new();
    if let Some(matcher) = spec.matcher {
        wrapper.insert("matcher".to_string(), Value::String(matcher.to_string()));
    }
    wrapper.insert("hooks".to_string(), Value::Array(vec![Value::Object(hook)]));
    Value::Object(wrapper)
}

/// Type-directed idempotency check.
///
/// For `BinarySubcmd`: matches if command contains the hooks binary exe name AND ends with the subcmd.
/// For `PythonFile`: matches if command contains `tools/amplihack/` AND the filename.
fn wrapper_matches(wrapper: &Value, spec: &HookSpec, hook_system: &str) -> bool {
    let Some(wrapper_obj) = wrapper.as_object() else {
        return false;
    };

    let matcher_matches = match spec.matcher {
        Some(expected) => wrapper_obj.get("matcher").and_then(Value::as_str) == Some(expected),
        None => !wrapper_obj.contains_key("matcher"),
    };
    if !matcher_matches {
        return false;
    }

    let command = wrapper_obj
        .get("hooks")
        .and_then(Value::as_array)
        .and_then(|entries| entries.first())
        .and_then(Value::as_object)
        .and_then(|hook| hook.get("command"))
        .and_then(Value::as_str);

    let Some(command) = command else {
        return false;
    };

    match &spec.cmd {
        HookCommandKind::BinarySubcmd { subcmd } => {
            // Command must reference the hooks binary name AND end with the subcmd
            command.contains("amplihack-hooks") && command.ends_with(subcmd)
        }
        HookCommandKind::PythonFile { file } => {
            // Command must reference tools/amplihack/ path AND contain the filename
            let _ = hook_system; // xpia hooks use a different path
            command.contains(&format!("tools/{hook_system}/")) && command.contains(file)
        }
    }
}

fn ensure_object(value: &mut Value) -> &mut Map<String, Value> {
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value converted to object")
}

fn ensure_array(value: &mut Value) -> &mut Vec<Value> {
    if !value.is_array() {
        *value = Value::Array(Vec::new());
    }
    value.as_array_mut().expect("value converted to array")
}

fn missing_hook_paths(system: &str, files: &[&str]) -> Result<Vec<String>> {
    let base = home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join(system)
        .join("hooks");
    let mut missing = Vec::new();
    for file in files {
        let path = base.join(file);
        if !path.exists() {
            missing.push(format!("{system}/{file} (expected at {})", path.display()));
        }
    }
    Ok(missing)
}

fn manifest_path() -> Result<PathBuf> {
    Ok(staging_claude_dir()?
        .join("install")
        .join("amplihack-manifest.json"))
}

fn read_manifest(path: &Path) -> Result<InstallManifest> {
    // TODO(hardening): validate that manifest path entries contain no path-traversal
    // sequences (e.g. "../../../etc") before use; file a follow-up issue for this.
    if !path.exists() {
        return Ok(InstallManifest::default());
    }
    let Ok(raw) = fs::read_to_string(path) else {
        tracing::debug!(
            "could not read manifest at {}: returning empty",
            path.display()
        );
        return Ok(InstallManifest::default());
    };
    // A corrupt manifest is treated as an empty one, triggering a clean reinstall.
    // The inspect_err call surfaces the parse error at debug log level so it is
    // visible in tracing output without failing the caller.
    let manifest = serde_json::from_str::<InstallManifest>(&raw)
        .inspect_err(|e| {
            tracing::debug!(
                "corrupt manifest at {}: {e} — treating as empty",
                path.display()
            )
        })
        .unwrap_or_default();
    Ok(manifest)
}

fn write_manifest(path: &Path, manifest: &InstallManifest) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    fs::write(path, serde_json::to_string_pretty(manifest)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn all_rel_dirs(claude_dir: &Path) -> Result<BTreeSet<String>> {
    let mut result = BTreeSet::new();
    if !claude_dir.exists() {
        return Ok(result);
    }
    for path in walk_dirs(claude_dir)? {
        let rel = path
            .strip_prefix(claude_dir)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        result.insert(if rel.is_empty() { ".".to_string() } else { rel });
    }
    Ok(result)
}

fn get_all_files_and_dirs(
    claude_dir: &Path,
    root_dirs: &[PathBuf],
) -> Result<(Vec<String>, Vec<String>)> {
    let mut files = Vec::new();
    let mut dirs = BTreeSet::new();
    for root in root_dirs {
        if !root.exists() {
            continue;
        }
        for entry in walk_all(root)? {
            let rel = entry
                .strip_prefix(claude_dir)
                .unwrap_or(&entry)
                .to_string_lossy()
                .replace('\\', "/");
            if entry.is_dir() {
                dirs.insert(rel);
            } else if entry.is_file() {
                files.push(rel);
            }
        }
    }
    files.sort();
    Ok((files, dirs.into_iter().collect()))
}

const MAX_WALK_DEPTH: usize = 64;

/// BFS directory walk with predicate-based inclusion, symlink guard, and depth limit.
///
/// Symlinks are never followed — entries identified as symlinks via `symlink_metadata()`
/// are silently skipped to prevent directory traversal attacks.
/// Traversal stops at `MAX_WALK_DEPTH` to guard against pathologically deep trees.
///
/// The `include` predicate receives each `DirEntry` and controls whether it appears
/// in the returned list.  Directories are always queued for traversal regardless of
/// whether `include` returns `true` for them.  The root itself is always included.
fn walk_bounded(root: &Path, include: impl Fn(&fs::DirEntry) -> bool) -> Result<Vec<PathBuf>> {
    let mut results = vec![root.to_path_buf()];
    let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), 0));

    while let Some((dir, depth)) = queue.pop_front() {
        if depth >= MAX_WALK_DEPTH {
            // Silently skip entries beyond the depth limit rather than failing the
            // entire walk; the limit protects against symlink loops and untrusted trees.
            continue;
        }
        for entry in
            fs::read_dir(&dir).with_context(|| format!("failed to read {}", dir.display()))?
        {
            let entry = entry?;
            // symlink_metadata() does not follow symlinks — use it to detect them.
            let meta = entry
                .path()
                .symlink_metadata()
                .with_context(|| format!("failed to stat {}", entry.path().display()))?;
            if meta.file_type().is_symlink() {
                continue; // never follow symlinks
            }
            if meta.is_dir() {
                queue.push_back((entry.path(), depth + 1));
            }
            if include(&entry) {
                results.push(entry.path());
            }
        }
    }
    Ok(results)
}

/// Return the root directory and all subdirectories (no files).
fn walk_dirs(root: &Path) -> Result<Vec<PathBuf>> {
    // DirEntry::file_type() does not follow symlinks; symlinks are already
    // filtered out by walk_bounded, so this predicate safely identifies real dirs.
    walk_bounded(root, |e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
}

/// Return the root directory and all entries within it (files and directories).
fn walk_all(root: &Path) -> Result<Vec<PathBuf>> {
    walk_bounded(root, |_| true)
}

/// Copy a directory recursively, skipping symlinks with a warning.
/// Device files, sockets, and FIFOs are skipped silently.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("failed to create {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let source = entry.path();
        let target = dst.join(entry.file_name());
        // Use entry.file_type() — symlink-safe (does not follow symlinks)
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            // Skip symlinks with a warning to prevent directory traversal attacks
            println!("  ⚠️  Skipping symlink: {}", source.display());
            continue;
        } else if kind.is_dir() {
            copy_dir_recursive(&source, &target)?;
        } else if kind.is_file() {
            fs::copy(&source, &target).with_context(|| {
                format!(
                    "failed to copy {} to {}",
                    source.display(),
                    target.display()
                )
            })?;
        }
        // Device files, sockets, FIFOs: silently skipped
    }
    Ok(())
}

fn set_hook_permissions(target_dir: &Path) -> Result<usize> {
    let mut updated = 0usize;
    for path in walk_all(target_dir)? {
        if path.is_file()
            && path.extension().and_then(|value| value.to_str()) == Some("py")
            && path
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                == Some("hooks")
        {
            set_script_permissions(&path)?;
            updated += 1;
        }
    }
    Ok(updated)
}

fn set_script_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata =
            fs::metadata(path).with_context(|| format!("failed to stat {}", path.display()))?;
        let mut perms = metadata.permissions();
        perms.set_mode(perms.mode() | 0o110);
        fs::set_permissions(path, perms)
            .with_context(|| format!("failed to chmod {}", path.display()))?;
    }
    Ok(())
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Returns `true` if the path has at least one executable bit set.
/// On non-Unix platforms every file is considered executable.
fn is_executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| !path.as_os_str().is_empty())
        .context("HOME is not set")
}

fn global_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".claude"))
}

fn staging_claude_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".amplihack").join(".claude"))
}

fn global_settings_path() -> Result<PathBuf> {
    Ok(global_claude_dir()?.join("settings.json"))
}

fn amplihack_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("amplihack")
        .join("hooks"))
}

fn xpia_hooks_dir() -> Result<PathBuf> {
    Ok(home_dir()?
        .join(".amplihack")
        .join(".claude")
        .join("tools")
        .join("xpia")
        .join("hooks"))
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── Shared test fixture ───────────────────────────────────────────────────

    /// Builds a minimal fake amplihack repository under `root`.
    fn create_source_repo(root: &Path) {
        for dir in ESSENTIAL_DIRS {
            fs::create_dir_all(root.join(".claude").join(dir)).unwrap();
        }
        fs::create_dir_all(root.join(".claude/tools/amplihack/hooks")).unwrap();
        for hook in AMPLIHACK_HOOK_FILES {
            fs::write(
                root.join(".claude/tools/amplihack/hooks").join(hook),
                "print(1)\n",
            )
            .unwrap();
        }
        fs::write(root.join(".claude/settings.json"), "{}\n").unwrap();
        fs::write(root.join(".claude/tools/statusline.sh"), "echo hi\n").unwrap();
        fs::write(root.join(".claude/AMPLIHACK.md"), "framework\n").unwrap();
        fs::write(root.join("CLAUDE.md"), "root\n").unwrap();
    }

    /// Creates an executable stub at `dir/name` (755 perms on Unix).
    /// Content is padded to > 1024 bytes so deploy_binaries size check passes.
    fn create_exe_stub(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let content = format!("#!/usr/bin/env bash\nexit 0\n{}\n", "x".repeat(1100));
        fs::write(&path, content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    // ─── Existing baseline tests (updated to use new InstallManifest struct API)

    /// UPDATED: uses new `InstallManifest` struct for write_manifest and
    ///          asserts binary-subcommand format in settings.json (not Python paths).
    ///
    /// FAILS until:
    ///   - `InstallManifest` gains `binaries` + `hook_registrations` fields
    ///   - `write_manifest` accepts `&InstallManifest`
    ///   - `local_install` uses binary-subcommand format for hook commands
    #[test]
    fn local_install_writes_manifest() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // Stub python3 + amplihack-hooks so validate_python / deploy_binaries succeed.
        let bin_dir = temp.path().join("stub_bin");
        fs::create_dir_all(&bin_dir).unwrap();
        create_exe_stub(&bin_dir, "python3");
        let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
            let new_path = format!(
                "{}:{}",
                bin_dir.display(),
                prev_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            std::env::set_var("PATH", &new_path);
        }

        create_source_repo(temp.path());
        local_install(temp.path()).unwrap();

        // Restore env
        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }

        // Manifest must exist
        assert!(
            temp.path()
                .join(".amplihack/.claude/install/amplihack-manifest.json")
                .exists()
        );
        // Hook files must be deployed
        assert!(
            temp.path()
                .join(".amplihack/.claude/tools/amplihack/hooks/pre_tool_use.py")
                .exists()
        );

        let settings = fs::read_to_string(temp.path().join(".claude/settings.json")).unwrap();
        // After implementation: settings.json uses binary-subcommand format, NOT Python file paths.
        assert!(
            settings.contains("amplihack-hooks"),
            "settings.json must reference amplihack-hooks binary, got:\n{settings}"
        );
        assert!(
            settings.contains("pre-tool-use"),
            "settings.json must reference 'pre-tool-use' subcommand, got:\n{settings}"
        );

        crate::test_support::restore_home(previous);
    }

    /// UPDATED: uses new `InstallManifest` struct for write_manifest call.
    ///
    /// FAILS until `write_manifest` accepts `&InstallManifest`.
    #[test]
    fn uninstall_removes_manifest_tracked_files() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        fs::create_dir_all(temp.path().join(".amplihack/.claude/install")).unwrap();
        fs::create_dir_all(temp.path().join(".amplihack/.claude/agents/amplihack")).unwrap();
        fs::write(
            temp.path()
                .join(".amplihack/.claude/agents/amplihack/demo.txt"),
            "x",
        )
        .unwrap();
        let manifest = InstallManifest {
            files: vec![String::from("agents/amplihack/demo.txt")],
            dirs: vec![String::from("agents/amplihack")],
            binaries: vec![],
            hook_registrations: vec![],
        };
        write_manifest(
            &temp
                .path()
                .join(".amplihack/.claude/install/amplihack-manifest.json"),
            &manifest,
        )
        .unwrap();
        run_uninstall().unwrap();
        assert!(
            !temp
                .path()
                .join(".amplihack/.claude/agents/amplihack")
                .exists()
        );
        crate::test_support::restore_home(previous);
    }

    /// UPDATED: uses new `read_manifest` → `Result<InstallManifest>` return type.
    ///
    /// FAILS until `read_manifest` returns `Result<InstallManifest>`.
    #[test]
    fn read_manifest_treats_invalid_json_as_empty() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("amplihack-manifest.json");
        fs::write(&path, "{invalid json\n").unwrap();

        let manifest = read_manifest(&path).unwrap();

        assert!(manifest.files.is_empty());
        assert!(manifest.dirs.is_empty());
        assert!(manifest.binaries.is_empty());
        assert!(manifest.hook_registrations.is_empty());
    }

    // ─── TDD: Group 1 — HookCommandKind discriminant ──────────────────────────

    /// FAILS until `HookCommandKind` enum is defined with `BinarySubcmd` variant.
    #[test]
    fn hook_command_kind_binary_subcmd_variant_exists() {
        let _kind = HookCommandKind::BinarySubcmd {
            subcmd: "session-start",
        };
    }

    /// FAILS until `HookCommandKind` enum is defined with `PythonFile` variant.
    #[test]
    fn hook_command_kind_python_file_variant_exists() {
        let _kind = HookCommandKind::PythonFile {
            file: "workflow_classification_reminder.py",
        };
    }

    // ─── TDD: Group 2 — AMPLIHACK_HOOK_SPECS canonical entries ───────────────

    /// FAILS until `HookSpec.cmd` is `HookCommandKind::BinarySubcmd { subcmd: "session-start" }`.
    #[test]
    fn amplihack_hook_specs_session_start_uses_binary_subcmd() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| s.event == "SessionStart")
            .expect("SessionStart spec must exist");
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => {
                assert_eq!(*subcmd, "session-start");
            }
            _ => panic!("SessionStart must use HookCommandKind::BinarySubcmd"),
        }
        assert_eq!(spec.timeout, Some(10));
        assert!(spec.matcher.is_none());
    }

    /// FAILS until Stop spec uses `BinarySubcmd { subcmd: "stop" }`.
    #[test]
    fn amplihack_hook_specs_stop_uses_binary_subcmd() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| s.event == "Stop")
            .expect("Stop spec must exist");
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => assert_eq!(*subcmd, "stop"),
            _ => panic!("Stop must use HookCommandKind::BinarySubcmd"),
        }
        assert_eq!(spec.timeout, Some(120));
        assert!(spec.matcher.is_none());
    }

    /// FAILS until PreToolUse spec uses `BinarySubcmd { subcmd: "pre-tool-use" }`.
    #[test]
    fn amplihack_hook_specs_pre_tool_use_uses_binary_subcmd() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| s.event == "PreToolUse")
            .expect("PreToolUse spec must exist");
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => assert_eq!(*subcmd, "pre-tool-use"),
            _ => panic!("PreToolUse must use HookCommandKind::BinarySubcmd"),
        }
        // PreToolUse must have NO timeout (fail-open per spec)
        assert!(
            spec.timeout.is_none(),
            "PreToolUse must omit timeout (fail-open)"
        );
        assert_eq!(spec.matcher, Some("*"));
    }

    /// FAILS until PostToolUse spec uses `BinarySubcmd { subcmd: "post-tool-use" }`.
    #[test]
    fn amplihack_hook_specs_post_tool_use_uses_binary_subcmd() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| s.event == "PostToolUse")
            .expect("PostToolUse spec must exist");
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => assert_eq!(*subcmd, "post-tool-use"),
            _ => panic!("PostToolUse must use HookCommandKind::BinarySubcmd"),
        }
        assert!(
            spec.timeout.is_none(),
            "PostToolUse must omit timeout (fail-open)"
        );
        assert_eq!(spec.matcher, Some("*"));
    }

    /// FAILS until workflow_classification_reminder uses `PythonFile` kind with 5s timeout.
    #[test]
    fn amplihack_hook_specs_workflow_classification_uses_python_file() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| {
                matches!(
                    &s.cmd,
                    HookCommandKind::PythonFile { file }
                        if *file == "workflow_classification_reminder.py"
                )
            })
            .expect("workflow_classification_reminder.py PythonFile spec must exist");
        assert_eq!(spec.event, "UserPromptSubmit");
        assert_eq!(spec.timeout, Some(5));
        assert!(spec.matcher.is_none());
    }

    /// FAILS until user-prompt-submit spec uses `BinarySubcmd { subcmd: "user-prompt-submit" }`.
    #[test]
    fn amplihack_hook_specs_user_prompt_submit_uses_binary_subcmd() {
        let specs: Vec<_> = AMPLIHACK_HOOK_SPECS
            .iter()
            .filter(|s| {
                s.event == "UserPromptSubmit"
                    && matches!(&s.cmd, HookCommandKind::BinarySubcmd { .. })
            })
            .collect();
        assert_eq!(
            specs.len(),
            1,
            "Exactly one UserPromptSubmit BinarySubcmd spec expected"
        );
        match &specs[0].cmd {
            HookCommandKind::BinarySubcmd { subcmd } => {
                assert_eq!(*subcmd, "user-prompt-submit")
            }
            _ => unreachable!(),
        }
        assert_eq!(specs[0].timeout, Some(10));
    }

    /// FAILS until PreCompact spec uses `BinarySubcmd { subcmd: "pre-compact" }`.
    #[test]
    fn amplihack_hook_specs_pre_compact_uses_binary_subcmd() {
        let spec = AMPLIHACK_HOOK_SPECS
            .iter()
            .find(|s| s.event == "PreCompact")
            .expect("PreCompact spec must exist");
        match &spec.cmd {
            HookCommandKind::BinarySubcmd { subcmd } => assert_eq!(*subcmd, "pre-compact"),
            _ => panic!("PreCompact must use HookCommandKind::BinarySubcmd"),
        }
        assert_eq!(spec.timeout, Some(30));
        assert!(spec.matcher.is_none());
    }

    /// FAILS until workflow_classification_reminder appears before user-prompt-submit in spec slice.
    ///
    /// Claude Code executes UserPromptSubmit hooks in array order; the Python stub
    /// (5 s budget) must run before the Rust binary (10 s budget).
    #[test]
    fn user_prompt_submit_workflow_classification_precedes_user_prompt_submit() {
        let python_pos = AMPLIHACK_HOOK_SPECS
            .iter()
            .position(|s| {
                matches!(
                    &s.cmd,
                    HookCommandKind::PythonFile { file }
                        if *file == "workflow_classification_reminder.py"
                )
            })
            .expect("PythonFile spec must exist");
        let binary_pos = AMPLIHACK_HOOK_SPECS
            .iter()
            .position(|s| {
                s.event == "UserPromptSubmit"
                    && matches!(
                        &s.cmd,
                        HookCommandKind::BinarySubcmd { subcmd }
                            if *subcmd == "user-prompt-submit"
                    )
            })
            .expect("BinarySubcmd user-prompt-submit must exist");
        assert!(
            python_pos < binary_pos,
            "workflow_classification_reminder (pos {python_pos}) must precede \
             user-prompt-submit (pos {binary_pos}) in AMPLIHACK_HOOK_SPECS"
        );
    }

    // ─── TDD: Group 3 — build_hook_wrapper generates correct command strings ──

    /// FAILS until `build_hook_wrapper` accepts `hooks_bin: &Path` and generates
    /// `"{hooks_bin} {subcmd}"` for `BinarySubcmd` kind.
    #[test]
    fn build_hook_wrapper_binary_subcmd_generates_binary_plus_subcmd() {
        let temp = tempfile::tempdir().unwrap();
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec = HookSpec {
            event: "SessionStart",
            cmd: HookCommandKind::BinarySubcmd {
                subcmd: "session-start",
            },
            timeout: Some(10),
            matcher: None,
        };

        let wrapper = build_hook_wrapper(&spec, &hooks_bin);
        let hooks_arr = wrapper["hooks"]
            .as_array()
            .expect("wrapper must have hooks[]");
        let hook = hooks_arr[0].as_object().expect("hooks[0] must be object");

        let command = hook["command"].as_str().expect("hook must have command");
        assert!(
            command.contains("amplihack-hooks"),
            "command must reference the hooks binary, got: {command}"
        );
        assert!(
            command.ends_with("session-start"),
            "command must end with subcommand 'session-start', got: {command}"
        );

        let timeout = hook["timeout"].as_u64().expect("hook must have timeout");
        assert_eq!(timeout, 10);

        assert!(
            !wrapper.as_object().unwrap().contains_key("matcher"),
            "wrapper must not have matcher for SessionStart"
        );
    }

    /// FAILS until PreToolUse wrapper omits `timeout` field and includes `matcher: "*"`.
    #[test]
    fn build_hook_wrapper_binary_subcmd_omits_timeout_when_none() {
        let temp = tempfile::tempdir().unwrap();
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec = HookSpec {
            event: "PreToolUse",
            cmd: HookCommandKind::BinarySubcmd {
                subcmd: "pre-tool-use",
            },
            timeout: None,
            matcher: Some("*"),
        };

        let wrapper = build_hook_wrapper(&spec, &hooks_bin);
        let hooks_arr = wrapper["hooks"].as_array().unwrap();
        let hook = hooks_arr[0].as_object().unwrap();

        assert!(
            !hook.contains_key("timeout"),
            "PreToolUse hook must NOT include timeout field"
        );
        assert_eq!(
            wrapper["matcher"].as_str().unwrap(),
            "*",
            "wrapper must include matcher"
        );
    }

    /// FAILS until `PythonFile` kind generates an absolute path to the `.py` file
    /// inside `tools/amplihack/hooks/`.
    #[test]
    fn build_hook_wrapper_python_file_generates_absolute_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec = HookSpec {
            event: "UserPromptSubmit",
            cmd: HookCommandKind::PythonFile {
                file: "workflow_classification_reminder.py",
            },
            timeout: Some(5),
            matcher: None,
        };

        let wrapper = build_hook_wrapper(&spec, &hooks_bin);
        let hooks_arr = wrapper["hooks"].as_array().unwrap();
        let hook = hooks_arr[0].as_object().unwrap();

        let command = hook["command"].as_str().expect("hook must have command");
        assert!(
            command
                .split('/')
                .collect::<Vec<_>>()
                .windows(2)
                .any(|w| w == ["tools", "amplihack"]),
            "PythonFile command must reference tools/amplihack/ dir, got: {command}"
        );
        assert!(
            command.ends_with("workflow_classification_reminder.py\""),
            "PythonFile command must end with quoted filename, got: {command}"
        );
        assert_eq!(hook["timeout"].as_u64().unwrap(), 5);

        crate::test_support::restore_home(previous);
    }

    // ─── TDD: Group 4 — wrapper_matches type-directed idempotency ────────────

    /// FAILS until `wrapper_matches` correctly identifies a BinarySubcmd wrapper.
    #[test]
    fn wrapper_matches_returns_true_for_matching_binary_subcmd_wrapper() {
        let temp = tempfile::tempdir().unwrap();
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec = HookSpec {
            event: "SessionStart",
            cmd: HookCommandKind::BinarySubcmd {
                subcmd: "session-start",
            },
            timeout: Some(10),
            matcher: None,
        };

        let wrapper = build_hook_wrapper(&spec, &hooks_bin);
        assert!(
            wrapper_matches(&wrapper, &spec, "amplihack"),
            "wrapper_matches must return true for an exact BinarySubcmd match"
        );
    }

    /// FAILS until `wrapper_matches` rejects a wrapper whose subcmd differs.
    #[test]
    fn wrapper_matches_returns_false_for_different_binary_subcmd() {
        let temp = tempfile::tempdir().unwrap();
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec_session = HookSpec {
            event: "SessionStart",
            cmd: HookCommandKind::BinarySubcmd {
                subcmd: "session-start",
            },
            timeout: Some(10),
            matcher: None,
        };
        let spec_stop = HookSpec {
            event: "Stop",
            cmd: HookCommandKind::BinarySubcmd { subcmd: "stop" },
            timeout: Some(120),
            matcher: None,
        };

        // Build wrapper for session-start; try to match with stop spec → should fail
        let wrapper = build_hook_wrapper(&spec_session, &hooks_bin);
        assert!(
            !wrapper_matches(&wrapper, &spec_stop, "amplihack"),
            "wrapper_matches must reject wrapper with different subcmd"
        );
    }

    /// FAILS until `wrapper_matches` correctly identifies a PythonFile wrapper
    /// via filename + `tools/amplihack/` containment.
    #[test]
    fn wrapper_matches_returns_true_for_python_file_wrapper() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());
        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let spec = HookSpec {
            event: "UserPromptSubmit",
            cmd: HookCommandKind::PythonFile {
                file: "workflow_classification_reminder.py",
            },
            timeout: Some(5),
            matcher: None,
        };

        let wrapper = build_hook_wrapper(&spec, &hooks_bin);
        assert!(
            wrapper_matches(&wrapper, &spec, "amplihack"),
            "wrapper_matches must return true for a PythonFile wrapper"
        );
        crate::test_support::restore_home(previous);
    }

    // ─── TDD: Group 5 — find_hooks_binary resolution ─────────────────────────

    /// FAILS until `find_hooks_binary` is defined and honours
    /// `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` env-var override.
    #[test]
    fn find_hooks_binary_uses_env_var_override_when_set() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let fake_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let prev = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &fake_bin);
        }

        let result = find_hooks_binary();

        // Restore
        if let Some(v) = prev {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }

        let resolved = result.expect("find_hooks_binary must resolve via env-var override");
        assert_eq!(
            resolved, fake_bin,
            "must return the exact path from env var"
        );
    }

    /// Verifies that `find_hooks_binary` returns an error (bails) when
    /// `AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH` is set to a non-existent path.
    #[test]
    fn find_hooks_binary_errors_when_env_var_path_nonexistent() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let nonexistent = temp.path().join("does-not-exist");

        let prev = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &nonexistent);
        }

        let result = find_hooks_binary();

        if let Some(v) = prev {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }

        assert!(
            result.is_err(),
            "find_hooks_binary must return an error when env var path does not exist"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH"),
            "error message must mention the env var name; got: {msg}"
        );
    }

    // ─── TDD: Group 6 — validate_hook_command_string ─────────────────────────

    /// FAILS until `validate_hook_command_string` is defined and rejects `|`.
    #[test]
    fn validate_hook_command_string_rejects_pipe() {
        assert!(
            validate_hook_command_string("/home/user/amplihack-hooks | evil").is_err(),
            "must reject pipe metacharacter"
        );
    }

    /// FAILS until `validate_hook_command_string` rejects `;`.
    #[test]
    fn validate_hook_command_string_rejects_semicolon() {
        assert!(
            validate_hook_command_string("/home/user/amplihack-hooks; rm -rf /").is_err(),
            "must reject semicolon"
        );
    }

    /// FAILS until `validate_hook_command_string` rejects `$`.
    #[test]
    fn validate_hook_command_string_rejects_dollar_sign() {
        assert!(
            validate_hook_command_string("/home/user/amplihack-hooks $HOME").is_err(),
            "must reject dollar-sign variable expansion"
        );
    }

    /// FAILS until `validate_hook_command_string` rejects backtick.
    #[test]
    fn validate_hook_command_string_rejects_backtick() {
        assert!(
            validate_hook_command_string("/home/user/amplihack-hooks `id`").is_err(),
            "must reject backtick"
        );
    }

    /// FAILS until `validate_hook_command_string` accepts a clean binary+subcmd string.
    #[test]
    fn validate_hook_command_string_accepts_valid_binary_subcmd() {
        assert!(
            validate_hook_command_string("/home/user/.local/bin/amplihack-hooks session-start")
                .is_ok(),
            "must accept plain binary + subcommand"
        );
    }

    /// FAILS until `validate_hook_command_string` accepts a clean Python file path.
    #[test]
    fn validate_hook_command_string_accepts_valid_python_path() {
        assert!(
            validate_hook_command_string(
                "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py"
            )
            .is_ok(),
            "must accept absolute Python file path"
        );
    }

    // ─── TDD: Group 7 — deploy_binaries ──────────────────────────────────────

    /// FAILS until `deploy_binaries` is defined and copies the hooks binary to
    /// `~/.local/bin` with 0o755 permissions.
    #[test]
    fn deploy_binaries_copies_hooks_binary_to_local_bin_with_755_perms() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let hooks_stub = create_exe_stub(temp.path(), "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
        }

        let result = deploy_binaries();

        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        crate::test_support::restore_home(previous);

        let deployed = result.expect("deploy_binaries must succeed");
        assert!(!deployed.is_empty(), "must return deployed paths");

        let dst = temp.path().join(".local/bin/amplihack-hooks");
        assert!(dst.exists(), "amplihack-hooks must be at ~/.local/bin");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&dst).unwrap().permissions().mode();
            assert_eq!(
                mode & 0o777,
                0o755,
                "deployed binary must have 0o755 perms, got {:03o}",
                mode & 0o777
            );
        }
    }

    /// FAILS until `deploy_binaries` returns Ok (warning, not error) when
    /// `~/.local/bin` is not in `$PATH`.
    #[test]
    fn deploy_binaries_succeeds_when_local_bin_not_in_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let hooks_stub = create_exe_stub(temp.path(), "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
            std::env::set_var("PATH", "/usr/bin:/bin"); // ~/.local/bin intentionally absent
        }

        let result = deploy_binaries();

        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        assert!(
            result.is_ok(),
            "deploy_binaries must exit 0 (warning only) even when ~/.local/bin absent from PATH"
        );
    }

    // ─── TDD: Group 8 — validate_python ──────────────────────────────────────

    /// FAILS until `validate_python` is defined and succeeds when a stub `python3`
    /// exits 0 (and `import amplihack` is treated as available).
    #[test]
    fn validate_python_succeeds_with_stub_python3() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        // Stub exits 0 for all invocations (including python3 -c 'import amplihack')
        create_exe_stub(temp.path(), "python3");

        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", temp.path().display(), orig_path.to_string_lossy());
        unsafe { std::env::set_var("PATH", &new_path) };

        let result = validate_python();

        unsafe { std::env::set_var("PATH", orig_path) };

        assert!(
            result.is_ok(),
            "validate_python must succeed when python3 stub exits 0"
        );
    }

    /// FAILS until `validate_python` returns Err when `python3` is absent from PATH.
    #[test]
    fn validate_python_fails_when_python3_not_in_path() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap(); // empty dir, no python3
        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        unsafe { std::env::set_var("PATH", temp.path()) };

        let result = validate_python();

        unsafe { std::env::set_var("PATH", orig_path) };

        assert!(
            result.is_err(),
            "validate_python must return Err when python3 is missing"
        );
    }

    /// FAILS until `validate_python` returns Err when `import amplihack` fails
    /// (python3 exits non-zero for the import check).
    #[test]
    fn validate_python_fails_when_amplihack_module_not_importable() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        // Stub exits 0 for --version but 1 for any `-c` arg (import check)
        let python3 = temp.path().join("python3");
        fs::write(
            &python3,
            "#!/usr/bin/env bash\nif [[ \"$*\" == *\"-c\"* ]]; then exit 1; fi\nexit 0\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&python3, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        let orig_path = std::env::var_os("PATH").unwrap_or_default();
        let new_path = format!("{}:{}", temp.path().display(), orig_path.to_string_lossy());
        unsafe { std::env::set_var("PATH", &new_path) };

        let result = validate_python();

        unsafe { std::env::set_var("PATH", orig_path) };

        assert!(
            result.is_err(),
            "validate_python must return Err when `import amplihack` fails"
        );
    }

    // ─── TDD: Group 9 — InstallManifest extended fields ──────────────────────

    /// FAILS until `InstallManifest` has `binaries` and `hook_registrations` fields.
    #[test]
    fn install_manifest_has_all_four_fields() {
        let manifest = InstallManifest {
            files: vec![String::from("a.py")],
            dirs: vec![String::from("dir")],
            binaries: vec![String::from("/home/user/.local/bin/amplihack-hooks")],
            hook_registrations: vec![String::from("SessionStart"), String::from("Stop")],
        };
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.dirs.len(), 1);
        assert_eq!(manifest.binaries.len(), 1);
        assert_eq!(manifest.hook_registrations.len(), 2);
    }

    /// FAILS until `InstallManifest` serialises with `binaries` and `hook_registrations`.
    #[test]
    fn install_manifest_serialises_new_fields() {
        let manifest = InstallManifest {
            files: vec![],
            dirs: vec![],
            binaries: vec![String::from("/home/user/.local/bin/amplihack-hooks")],
            hook_registrations: vec![String::from("SessionStart")],
        };
        let json = serde_json::to_string(&manifest).unwrap();
        assert!(
            json.contains("\"binaries\""),
            "serialised manifest must contain 'binaries'"
        );
        assert!(
            json.contains("\"hook_registrations\""),
            "serialised manifest must contain 'hook_registrations'"
        );
    }

    /// FAILS until `InstallManifest` has `#[serde(default)]` on new fields so old
    /// manifests (without those keys) still deserialise successfully.
    #[test]
    fn install_manifest_deserialises_old_format_with_empty_defaults() {
        let old_json = r#"{"files": ["a.py"], "dirs": ["dir"]}"#;
        let manifest: InstallManifest =
            serde_json::from_str(old_json).expect("must deserialise old 2-field format");
        assert_eq!(manifest.files, vec!["a.py"]);
        assert!(
            manifest.binaries.is_empty(),
            "binaries must default to [] for old manifests"
        );
        assert!(
            manifest.hook_registrations.is_empty(),
            "hook_registrations must default to [] for old manifests"
        );
    }

    // ─── TDD: Group 10 — create_runtime_dirs sets 0o755 ─────────────────────

    /// FAILS until `create_runtime_dirs` calls `set_permissions(0o755)` on each
    /// directory it creates.
    #[test]
    fn create_runtime_dirs_applies_0o755_permissions() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let staging_dir = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(&staging_dir).unwrap();
        create_runtime_dirs(&staging_dir).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for dir in RUNTIME_DIRS {
                let full = staging_dir.join(dir);
                assert!(full.exists(), "runtime dir '{dir}' must be created");
                let mode = fs::metadata(&full).unwrap().permissions().mode();
                assert_eq!(
                    mode & 0o777,
                    0o755,
                    "runtime dir '{dir}' must have 0o755 perms, got {:03o}",
                    mode & 0o777
                );
            }
        }

        crate::test_support::restore_home(previous);
    }

    // ─── TDD: Group 11 — copy_dir_recursive symlink safety ───────────────────

    /// FAILS until `copy_dir_recursive` skips symlinks rather than following them.
    ///
    /// A crafted repo with a symlink pointing outside the source tree must not
    /// expose sensitive content through a directory traversal.
    #[test]
    fn copy_dir_recursive_skips_symlinks_without_following() {
        let temp = tempfile::tempdir().unwrap();
        let src = temp.path().join("src");
        let dst = temp.path().join("dst");
        fs::create_dir_all(&src).unwrap();

        fs::write(src.join("real.txt"), "content").unwrap();

        #[cfg(unix)]
        {
            let outside = temp.path().join("outside.txt");
            fs::write(&outside, "sensitive-data").unwrap();
            std::os::unix::fs::symlink(&outside, src.join("evil_link.txt")).unwrap();
        }

        copy_dir_recursive(&src, &dst).unwrap();

        // Regular files must be copied
        assert!(dst.join("real.txt").exists(), "real.txt must be copied");

        // Symlink must not be followed to expose sensitive content
        #[cfg(unix)]
        {
            let sym_dst = dst.join("evil_link.txt");
            if sym_dst.exists() {
                let content = fs::read_to_string(&sym_dst).unwrap_or_default();
                assert_ne!(
                    content, "sensitive-data",
                    "symlink must not be followed; sensitive content must not be copied"
                );
            }
            // Per spec: symlinks are skipped entirely; the destination must not be
            // a regular file even if the symlink path exists in dst.
            assert!(
                !sym_dst.is_file() || sym_dst.is_symlink(),
                "evil_link.txt in dst must not be a regular file"
            );
        }
    }

    // ─── TDD: Group 12 — ensure_settings_json returns (bool, Vec<String>) ────

    /// FAILS until `ensure_settings_json` signature changes to
    /// `(staging_dir: &Path, timestamp: u64, hooks_bin: &Path) -> Result<(bool, Vec<String>)>`.
    #[test]
    fn ensure_settings_json_returns_registered_event_names() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // Populate staging dir with all required hook files
        let staging_dir = temp.path().join(".amplihack/.claude");
        let hooks_dir = staging_dir.join("tools/amplihack/hooks");
        fs::create_dir_all(&hooks_dir).unwrap();
        for hook in AMPLIHACK_HOOK_FILES {
            fs::write(hooks_dir.join(hook), "print(1)\n").unwrap();
        }

        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_bin);
        }

        let result = ensure_settings_json(&staging_dir, 99999, &hooks_bin);

        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        crate::test_support::restore_home(previous);

        let (success, events) = result.expect("ensure_settings_json must not error");
        assert!(success, "must return true when hooks are present");
        assert!(!events.is_empty(), "must return non-empty event list");

        // All canonical event types must be represented
        for expected in [
            "SessionStart",
            "Stop",
            "PreToolUse",
            "PostToolUse",
            "UserPromptSubmit",
            "PreCompact",
        ] {
            assert!(
                events.contains(&expected.to_string()),
                "events must include '{expected}', got: {events:?}"
            );
        }
    }

    // ─── TDD: Group 13 — local_install writes 4-field manifest ───────────────

    /// FAILS until `local_install` populates `binaries` and `hook_registrations`
    /// in the manifest it writes.
    #[test]
    fn local_install_writes_manifest_with_all_four_fields() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let bin_dir = temp.path().join("stub_bin");
        fs::create_dir_all(&bin_dir).unwrap();
        create_exe_stub(&bin_dir, "python3");
        let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
            let new_path = format!(
                "{}:{}",
                bin_dir.display(),
                prev_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            std::env::set_var("PATH", &new_path);
        }

        create_source_repo(temp.path());
        local_install(temp.path()).unwrap();

        // Restore env
        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        let manifest_path = temp
            .path()
            .join(".amplihack/.claude/install/amplihack-manifest.json");
        assert!(manifest_path.exists());

        let raw = fs::read_to_string(&manifest_path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert!(json.get("files").is_some(), "manifest must have 'files'");
        assert!(json.get("dirs").is_some(), "manifest must have 'dirs'");
        assert!(
            json.get("binaries").is_some(),
            "manifest must have 'binaries'"
        );
        assert!(
            json.get("hook_registrations").is_some(),
            "manifest must have 'hook_registrations'"
        );

        let binaries = json["binaries"].as_array().unwrap();
        assert!(
            !binaries.is_empty(),
            "manifest.binaries must be non-empty after install"
        );

        let hook_regs = json["hook_registrations"].as_array().unwrap();
        assert!(
            !hook_regs.is_empty(),
            "manifest.hook_registrations must be non-empty after install"
        );
    }

    // ─── TDD: Group 14 — run_install accepts local: Option<PathBuf> ──────────

    /// FAILS until `run_install` signature changes to accept `Option<PathBuf>` and
    /// uses the local path directly (skipping git clone).
    #[test]
    fn run_install_with_local_path_skips_git_clone() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let bin_dir = temp.path().join("stub_bin");
        fs::create_dir_all(&bin_dir).unwrap();
        create_exe_stub(&bin_dir, "python3");
        let hooks_stub = create_exe_stub(&bin_dir, "amplihack-hooks");

        let prev_hooks = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", &hooks_stub);
            let new_path = format!(
                "{}:{}",
                bin_dir.display(),
                prev_path
                    .as_ref()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default()
            );
            std::env::set_var("PATH", &new_path);
        }

        // Create local repo without git
        let local_repo = temp.path().join("local-repo");
        fs::create_dir_all(&local_repo).unwrap();
        create_source_repo(&local_repo);

        let result = run_install(Some(local_repo));

        if let Some(v) = prev_hooks {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        result.unwrap();

        assert!(
            temp.path()
                .join(".amplihack/.claude/install/amplihack-manifest.json")
                .exists(),
            "manifest must exist after --local install (no git required)"
        );
    }

    /// FAILS until `run_install` validates that the `--local` path exists.
    #[test]
    fn run_install_with_nonexistent_local_path_returns_err() {
        let nonexistent = PathBuf::from("/nonexistent/amplihack-repo/does-not-exist");
        let result = run_install(Some(nonexistent));
        assert!(
            result.is_err(),
            "run_install must return Err for a non-existent --local path"
        );
    }

    // ─── TDD: Group 15 — run_uninstall removes binaries (Phase 3) ────────────

    /// FAILS until `run_uninstall` reads `binaries` from the manifest and removes
    /// each binary file (Phase 3).
    #[test]
    fn run_uninstall_removes_binaries_listed_in_manifest() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // Place a fake binary in ~/.local/bin
        let local_bin = temp.path().join(".local/bin");
        fs::create_dir_all(&local_bin).unwrap();
        let hooks_binary = local_bin.join("amplihack-hooks");
        fs::write(&hooks_binary, "#!/bin/bash\n").unwrap();
        assert!(hooks_binary.exists());

        // Write a manifest that tracks the binary
        fs::create_dir_all(temp.path().join(".amplihack/.claude/install")).unwrap();
        let manifest = serde_json::json!({
            "files": [],
            "dirs": [],
            "binaries": [hooks_binary.to_string_lossy()],
            "hook_registrations": []
        });
        fs::write(
            temp.path()
                .join(".amplihack/.claude/install/amplihack-manifest.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();

        run_uninstall().unwrap();

        crate::test_support::restore_home(previous);

        assert!(
            !hooks_binary.exists(),
            "amplihack-hooks must be removed by uninstall Phase 3"
        );
    }

    // ─── TDD: Group 16 — remove_hook_registrations ───────────────────────────

    /// FAILS until `remove_hook_registrations` is defined and removes hook entries
    /// whose command contains `amplihack-hooks`.
    #[test]
    fn remove_hook_registrations_removes_amplihack_hooks_entries() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");

        let settings = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/amplihack-hooks session-start",
                            "timeout": 10
                        }]
                    },
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/some-other-tool start",
                            "timeout": 10
                        }]
                    }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        remove_hook_registrations(&settings_path).unwrap();

        let updated_raw = fs::read_to_string(&settings_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

        let session_hooks = updated["hooks"]["SessionStart"].as_array().unwrap();

        // amplihack-hooks entries must be gone
        for wrapper in session_hooks {
            if let Some(hooks) = wrapper.get("hooks").and_then(serde_json::Value::as_array) {
                for hook in hooks {
                    let cmd = hook["command"].as_str().unwrap_or("");
                    assert!(
                        !cmd.contains("amplihack-hooks"),
                        "amplihack-hooks command must be removed, found: {cmd}"
                    );
                }
            }
        }

        // Non-amplihack entry must still be present
        assert_eq!(
            session_hooks.len(),
            1,
            "non-amplihack hook must remain; only amplihack-hooks entry removed"
        );
    }

    /// FAILS until `remove_hook_registrations` also removes Python-path entries
    /// that contain `tools/amplihack/`.
    #[test]
    fn remove_hook_registrations_removes_tools_amplihack_python_paths() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");

        let settings = serde_json::json!({
            "hooks": {
                "UserPromptSubmit": [
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/workflow_classification_reminder.py",
                            "timeout": 5
                        }]
                    }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        remove_hook_registrations(&settings_path).unwrap();

        let updated_raw = fs::read_to_string(&settings_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

        // After Phase 2 pruning the UserPromptSubmit key is removed entirely
        // because its array became empty.  Both outcomes (absent key OR empty
        // array) mean no amplihack entries remain — test for either case.
        let any_amplihack_path = match updated["hooks"]["UserPromptSubmit"].as_array() {
            None => false, // key pruned — no entries remain
            Some(hooks_arr) => hooks_arr.iter().any(|wrapper| {
                wrapper
                    .get("hooks")
                    .and_then(serde_json::Value::as_array)
                    .map(|hooks| {
                        hooks.iter().any(|h| {
                            h["command"]
                                .as_str()
                                .map(|c| c.contains("tools/amplihack/"))
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false)
            }),
        };
        assert!(
            !any_amplihack_path,
            "tools/amplihack/ Python hook paths must be removed from settings.json"
        );
    }

    /// FAILS until `remove_hook_registrations` preserves non-amplihack entries
    /// (e.g. XPIA hooks must survive uninstall of the amplihack hooks).
    #[test]
    fn remove_hook_registrations_preserves_non_amplihack_entries() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");

        let settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "*",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.amplihack/.claude/tools/amplihack/hooks/pre_tool_use.py"
                        }]
                    },
                    {
                        "matcher": "*",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.amplihack/.claude/tools/xpia/hooks/pre_tool_use.py"
                        }]
                    }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        remove_hook_registrations(&settings_path).unwrap();

        let updated_raw = fs::read_to_string(&settings_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

        let hooks_arr = updated["hooks"]["PreToolUse"].as_array().unwrap();

        // XPIA entry must still be present
        let xpia_present = hooks_arr.iter().any(|wrapper| {
            wrapper
                .get("hooks")
                .and_then(serde_json::Value::as_array)
                .map(|hooks| {
                    hooks.iter().any(|h| {
                        h["command"]
                            .as_str()
                            .map(|c| c.contains("tools/xpia/"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        });
        assert!(
            xpia_present,
            "XPIA hook entries must NOT be removed by remove_hook_registrations"
        );
    }

    // ─── TDD: Group 16b — remove_hook_registrations prunes empty arrays ─────────

    /// FAILS until `remove_hook_registrations` removes event-type keys from the
    /// hooks map when their wrapper array becomes empty after amplihack hooks are
    /// removed.  Without this fix, settings.json ends up with entries like
    /// `"PreToolUse": []` which is visual noise and may confuse Claude Code.
    ///
    /// Acceptance criteria (issue #38):
    ///   - After uninstall, no event-type key in hooks map has an empty array value
    ///   - Keys whose arrays still have non-amplihack entries are preserved as-is
    #[test]
    fn remove_hook_registrations_leaves_no_empty_arrays() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");

        // Both events have ONLY amplihack hooks → both arrays become empty after removal
        let settings = serde_json::json!({
            "hooks": {
                "PreToolUse": [
                    {
                        "matcher": "*",
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/amplihack-hooks pre-tool-use"
                        }]
                    }
                ],
                "SessionStart": [
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/amplihack-hooks session-start",
                            "timeout": 10
                        }]
                    }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        remove_hook_registrations(&settings_path).unwrap();

        let updated_raw = fs::read_to_string(&settings_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

        // The hooks map must not contain any key whose value is an empty array.
        if let Some(hooks_map) = updated["hooks"].as_object() {
            for (event, wrappers_val) in hooks_map {
                if let Some(arr) = wrappers_val.as_array() {
                    assert!(
                        !arr.is_empty(),
                        "Event type '{}' must be removed from hooks map when all its \
                         wrappers are gone, but found empty array. Full hooks: {}",
                        event,
                        serde_json::to_string_pretty(&updated["hooks"]).unwrap()
                    );
                }
            }
        }
    }

    /// Verify that a mixed event (amplihack + non-amplihack wrappers) still retains
    /// the non-amplihack wrapper and does NOT produce an empty array.
    #[test]
    fn remove_hook_registrations_mixed_event_keeps_non_amplihack_wrapper() {
        let temp = tempfile::tempdir().unwrap();
        let settings_path = temp.path().join("settings.json");

        let settings = serde_json::json!({
            "hooks": {
                "PostToolUse": [
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/amplihack-hooks post-tool-use"
                        }]
                    },
                    {
                        "hooks": [{
                            "type": "command",
                            "command": "/home/user/.local/bin/third-party-tool post"
                        }]
                    }
                ]
            }
        });
        fs::write(&settings_path, serde_json::to_string(&settings).unwrap()).unwrap();

        remove_hook_registrations(&settings_path).unwrap();

        let updated_raw = fs::read_to_string(&settings_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated_raw).unwrap();

        // PostToolUse must still exist with one entry (the third-party wrapper)
        let wrappers = updated["hooks"]["PostToolUse"].as_array().unwrap();
        assert_eq!(
            wrappers.len(),
            1,
            "PostToolUse must retain the non-amplihack wrapper"
        );

        // The remaining wrapper must reference the third-party tool
        let cmd = wrappers[0]["hooks"][0]["command"].as_str().unwrap_or("");
        assert!(
            cmd.contains("third-party-tool"),
            "Remaining wrapper must be the third-party hook, got: {cmd}"
        );
    }

    // ─── TDD: Group 18 — find_hooks_binary lookup order (Issue #74 regression guard) ──

    /// Verifies that PATH lookup (Step 3) wins over `~/.local/bin` (Step 4) in
    /// `find_hooks_binary`, fixing Issue #74.
    ///
    /// After `amplihack uninstall` removes `~/.local/bin/amplihack-hooks`, the binary
    /// that the user originally placed in `/usr/local/bin` (or any PATH directory) must
    /// still be found on reinstall.  This test places a stub only in a synthetic PATH
    /// directory (not in `~/.local/bin`) and asserts that `find_hooks_binary` returns
    /// the PATH entry — confirming the new lookup order: env var → sibling → PATH →
    /// ~/.local/bin → ~/.cargo/bin.
    #[test]
    fn find_hooks_binary_path_lookup_wins_over_local_bin() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // ~/.local/bin has a binary (e.g. from a previous install)
        let local_bin = temp.path().join(".local").join("bin");
        fs::create_dir_all(&local_bin).unwrap();
        create_exe_stub(&local_bin, "amplihack-hooks");

        // A system PATH dir (e.g. /usr/local/bin) also has the binary
        let path_bin = temp.path().join("path_bin");
        fs::create_dir_all(&path_bin).unwrap();
        let path_stub = create_exe_stub(&path_bin, "amplihack-hooks");

        let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
            // PATH contains the system dir but NOT ~/.local/bin — simulates reinstall
            // after uninstall removed the ~/.local/bin copy while /usr/local/bin remains.
            std::env::set_var("PATH", &path_bin);
        }

        let result = find_hooks_binary();

        if let Some(v) = prev_env {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        let resolved = result.expect("find_hooks_binary must find the binary");
        assert_eq!(
            resolved, path_stub,
            "PATH lookup (Step 3) must win — find_hooks_binary returned {resolved:?} instead of {path_stub:?}"
        );
    }

    /// Reinstall scenario: uninstall has removed ~/.local/bin/amplihack-hooks but the
    /// system-wide binary at /usr/local/bin is still present (and in PATH).
    /// find_hooks_binary must recover via the PATH lookup at Step 3.
    #[test]
    fn find_hooks_binary_reinstall_after_uninstall_removes_local_bin() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // ~/.local/bin exists but has NO amplihack-hooks (it was deleted by uninstall)
        let local_bin = temp.path().join(".local").join("bin");
        fs::create_dir_all(&local_bin).unwrap();

        // System /usr/local/bin equivalent has the binary
        let usr_local_bin = temp.path().join("usr_local_bin");
        fs::create_dir_all(&usr_local_bin).unwrap();
        let system_stub = create_exe_stub(&usr_local_bin, "amplihack-hooks");

        let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
            // PATH contains the system dir (simulates /usr/local/bin being in PATH)
            std::env::set_var("PATH", &usr_local_bin);
        }

        let result = find_hooks_binary();

        if let Some(v) = prev_env {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        let resolved = result.expect(
            "find_hooks_binary must find the binary via PATH even when ~/.local/bin copy was removed by uninstall",
        );
        assert_eq!(
            resolved, system_stub,
            "reinstall must find system binary via PATH — got {resolved:?} instead of {system_stub:?}"
        );
    }

    /// Verifies that `~/.cargo/bin` is used as a fallback when `~/.local/bin` has
    /// no binary.  This closes the coverage gap for the Step 4 lookup chain.
    #[test]
    fn find_hooks_binary_falls_through_to_cargo_bin_when_local_bin_absent() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // ~/.local/bin exists but contains NO amplihack-hooks
        let local_bin = temp.path().join(".local").join("bin");
        fs::create_dir_all(&local_bin).unwrap();

        // ~/.cargo/bin has the binary
        let cargo_bin = temp.path().join(".cargo").join("bin");
        fs::create_dir_all(&cargo_bin).unwrap();
        let cargo_stub = create_exe_stub(&cargo_bin, "amplihack-hooks");

        let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
            // Empty PATH so PATH lookup (Step 5) also fails
            std::env::set_var("PATH", temp.path().join("empty_bin"));
        }

        let result = find_hooks_binary();

        if let Some(v) = prev_env {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        let resolved = result.expect("find_hooks_binary must fall through to ~/.cargo/bin");
        assert_eq!(
            resolved, cargo_stub,
            "~/.cargo/bin must be used when ~/.local/bin has no binary"
        );
    }

    /// Verifies that `find_hooks_binary` returns an informative error mentioning
    /// `amplihack-hooks` when the binary cannot be found by any lookup step.
    #[test]
    fn find_hooks_binary_returns_err_with_helpful_message_when_not_found() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        let prev_env = std::env::var_os("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
        let prev_path = std::env::var_os("PATH");
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH");
            // Empty/nonexistent PATH to ensure all resolution steps fail
            std::env::set_var("PATH", temp.path().join("empty_bin"));
        }

        let result = find_hooks_binary();

        if let Some(v) = prev_env {
            unsafe { std::env::set_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH", v) };
        } else {
            unsafe { std::env::remove_var("AMPLIHACK_AMPLIHACK_HOOKS_BINARY_PATH") };
        }
        if let Some(v) = prev_path {
            unsafe { std::env::set_var("PATH", v) };
        }
        crate::test_support::restore_home(previous);

        let err = result.expect_err("find_hooks_binary must return Err when binary is absent");
        let msg = format!("{err}");
        assert!(
            msg.contains("amplihack-hooks"),
            "error message must mention 'amplihack-hooks' to guide the user, got: {msg}"
        );
    }

    // ─── TDD: Group 19 — run_uninstall dedup correctness ─────────────────────

    /// Verifies that `run_uninstall` correctly handles manifests with duplicate
    /// directory entries.  The BTreeSet → sort_unstable+dedup replacement must
    /// produce the same result: each directory is attempted for removal exactly once.
    #[test]
    fn run_uninstall_handles_duplicate_dirs_in_manifest() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // Create the directory that the manifest will track
        let staging = temp.path().join(".amplihack/.claude");
        let tracked_dir = staging.join("agents/amplihack");
        fs::create_dir_all(&tracked_dir).unwrap();
        fs::write(tracked_dir.join("dummy.txt"), "x").unwrap();

        // Write a manifest with three copies of the same dir entry (simulating
        // a buggy or hand-crafted manifest that contains duplicates).
        fs::create_dir_all(staging.join("install")).unwrap();
        let manifest = InstallManifest {
            files: vec![],
            dirs: vec![
                "agents/amplihack".to_string(),
                "agents/amplihack".to_string(),
                "agents/amplihack".to_string(),
            ],
            binaries: vec![],
            hook_registrations: vec![],
        };
        write_manifest(&staging.join("install/amplihack-manifest.json"), &manifest).unwrap();

        // Must not error even though the dir is listed three times
        let result = run_uninstall();

        crate::test_support::restore_home(previous);

        assert!(
            result.is_ok(),
            "run_uninstall must succeed with duplicate dir entries in manifest, got: {result:?}"
        );
        assert!(
            !tracked_dir.exists(),
            "tracked directory must be removed during uninstall"
        );
    }

    // ─── TDD: Group 17 — backup metadata uses serde_json (not format strings) ─

    /// FAILS until `ensure_settings_json` writes backup metadata using
    /// `serde_json::json!` so the output is always valid JSON even when the
    /// settings path contains quote characters.
    #[test]
    fn backup_metadata_is_always_valid_json() {
        let _guard = crate::test_support::home_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let previous = crate::test_support::set_home(temp.path());

        // Pre-create a settings.json so a backup is triggered
        fs::create_dir_all(temp.path().join(".claude")).unwrap();
        fs::write(temp.path().join(".claude/settings.json"), "{}").unwrap();

        let staging_dir = temp.path().join(".amplihack/.claude");
        fs::create_dir_all(&staging_dir).unwrap();

        let hooks_bin = create_exe_stub(temp.path(), "amplihack-hooks");

        let timestamp = 1_700_000_000_u64;
        // ignore result — we only care that the backup metadata file is valid JSON
        let _ = ensure_settings_json(&staging_dir, timestamp, &hooks_bin);

        crate::test_support::restore_home(previous);

        let metadata_path = staging_dir
            .join("runtime/sessions")
            .join(format!("install_{timestamp}_backup.json"));

        if metadata_path.exists() {
            let raw = fs::read_to_string(&metadata_path).unwrap();
            let parsed: Result<serde_json::Value, _> = serde_json::from_str(&raw);
            assert!(
                parsed.is_ok(),
                "backup metadata must be valid JSON, got:\n{raw}"
            );
            let meta = parsed.unwrap();
            assert!(
                meta.get("settings_path").is_some(),
                "backup metadata must have 'settings_path'"
            );
            assert!(
                meta.get("backup_path").is_some(),
                "backup metadata must have 'backup_path'"
            );
        }
    }
}
