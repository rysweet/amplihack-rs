//! Blarify code-graph setup and indexing.

use amplihack_cli::binary_finder::BinaryFinder;
use amplihack_cli::memory::{
    background_index_job_active, check_index_status, resolve_code_graph_db_path_for_project,
};
use amplihack_types::ProjectDirs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub(super) fn setup_blarify_indexing(dirs: &ProjectDirs) -> anyhow::Result<BlarifySetupResult> {
    if background_index_job_active(&dirs.root)? {
        return Ok(BlarifySetupResult::with_notice(
            true,
            super::format_code_graph_status(
                "A background code-graph refresh is already running for this project. \
                 The code graph may be unavailable or locked until it finishes. \
                 Retry `amplihack query-code stats` after it completes."
                    .to_string(),
            ),
        ));
    }

    let status = check_index_status(&dirs.root)?;
    let db_path = resolve_code_graph_db_path_for_project(&dirs.root)?;
    let code_graph_missing = !db_path.exists();
    let needs_setup = status.needs_indexing || code_graph_missing;
    if !needs_setup {
        return Ok(BlarifySetupResult::ready());
    }

    if std::env::var("AMPLIHACK_DISABLE_BLARIFY").as_deref() == Ok("1") {
        return Ok(BlarifySetupResult::with_notice(
            false,
            super::format_code_graph_status(format!(
                "Automatic code-graph refresh is disabled by `AMPLIHACK_DISABLE_BLARIFY=1`, \
                 but setup is still needed because {}. The current code graph may be missing or stale.",
                describe_blarify_need(&status, code_graph_missing)
            )),
        ));
    }

    if !status.needs_indexing && !code_graph_missing {
        return Ok(BlarifySetupResult::ready());
    }

    let action = resolve_blarify_index_action(&status, &blarify_json_path(&dirs.root));
    match blarify_mode() {
        SessionStartBlarifyMode::Skip => Ok(BlarifySetupResult::with_notice(
            false,
            super::format_code_graph_status(format!(
                "Code-graph setup was needed because {}, but `AMPLIHACK_BLARIFY_MODE=skip` \
                 prevented it from running. Skipped action: {}. The current code graph may be missing or stale.",
                describe_blarify_need(&status, code_graph_missing),
                describe_blarify_action(action)
            )),
        )),
        SessionStartBlarifyMode::Sync => {
            run_blarify_indexing(&dirs.root, action, false, &db_path)?;
            Ok(BlarifySetupResult::ready())
        }
        SessionStartBlarifyMode::Background => {
            run_blarify_indexing(&dirs.root, action, true, &db_path)?;
            Ok(BlarifySetupResult::with_notice(
                true,
                super::format_code_graph_status(format!(
                    "Started background code-graph setup because {}. Planned action: {}. \
                     The code graph may be unavailable or locked until it finishes. \
                     Retry `amplihack query-code stats` after it completes.",
                    describe_blarify_need(&status, code_graph_missing),
                    describe_blarify_action(action)
                )),
            ))
        }
    }
}

fn run_blarify_indexing(
    project_root: &Path,
    action: BlarifyIndexAction,
    background: bool,
    db_path: &Path,
) -> anyhow::Result<()> {
    let amplihack = find_amplihack_binary()?;
    let mut cmd = build_blarify_index_command(&amplihack, project_root, action, db_path)?;
    cmd.current_dir(project_root);
    if background {
        let child = cmd
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        amplihack_cli::memory::record_background_index_pid(project_root, child.id())?;
    } else {
        let status = cmd.status()?;
        if !status.success() {
            anyhow::bail!("blarify indexing command failed with status {status}");
        }
    }
    Ok(())
}

fn build_blarify_index_command(
    amplihack_binary: &Path,
    project_root: &Path,
    action: BlarifyIndexAction,
    db_path: &Path,
) -> anyhow::Result<Command> {
    let mut cmd = Command::new(amplihack_binary);
    match action {
        BlarifyIndexAction::ImportExistingJson => {
            cmd.arg("index-code")
                .arg(blarify_json_path(project_root))
                .arg("--db-path")
                .arg(db_path);
        }
        BlarifyIndexAction::GenerateNativeScip => {
            cmd.arg("index-scip")
                .arg("--project-path")
                .arg(project_root);
        }
    }
    Ok(cmd)
}

fn find_amplihack_binary() -> anyhow::Result<PathBuf> {
    Ok(BinaryFinder::find("amplihack")?.path)
}

fn blarify_json_path(project_root: &Path) -> PathBuf {
    project_root.join(".amplihack").join("blarify.json")
}

fn resolve_blarify_index_action(
    status: &amplihack_cli::memory::IndexStatus,
    json_path: &Path,
) -> BlarifyIndexAction {
    if json_path.exists() && !status.needs_indexing {
        BlarifyIndexAction::ImportExistingJson
    } else {
        BlarifyIndexAction::GenerateNativeScip
    }
}

fn blarify_mode() -> SessionStartBlarifyMode {
    match std::env::var("AMPLIHACK_BLARIFY_MODE")
        .unwrap_or_else(|_| "background".to_string())
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "skip" => SessionStartBlarifyMode::Skip,
        "sync" => SessionStartBlarifyMode::Sync,
        _ => SessionStartBlarifyMode::Background,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlarifyIndexAction {
    ImportExistingJson,
    GenerateNativeScip,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionStartBlarifyMode {
    Skip,
    Sync,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct BlarifySetupResult {
    pub(super) indexing_active: bool,
    pub(super) status_context: Option<String>,
}

impl BlarifySetupResult {
    pub(super) fn ready() -> Self {
        Self {
            indexing_active: false,
            status_context: None,
        }
    }

    pub(super) fn with_notice(indexing_active: bool, status_context: String) -> Self {
        Self {
            indexing_active,
            status_context: Some(status_context),
        }
    }
}

fn describe_blarify_need(
    status: &amplihack_cli::memory::IndexStatus,
    code_graph_missing: bool,
) -> String {
    match (status.needs_indexing, code_graph_missing) {
        (true, true) => format!(
            "{} and the project code-graph database is missing",
            status.reason
        ),
        (true, false) => status.reason.clone(),
        (false, true) => "the project code-graph database is missing".to_string(),
        (false, false) => "no refresh is required".to_string(),
    }
}

fn describe_blarify_action(action: BlarifyIndexAction) -> &'static str {
    match action {
        BlarifyIndexAction::ImportExistingJson => {
            "import the current Blarify JSON into the project code-graph database"
        }
        BlarifyIndexAction::GenerateNativeScip => {
            "rebuild the project code graph with native SCIP indexing"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::env_lock;
    use amplihack_types::ProjectDirs;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    #[test]
    fn setup_blarify_indexing_background_imports_current_json_when_db_missing() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        fs::write(src_dir.join("app.py"), "print('hi')\n").unwrap();
        let artifact_dir = dir.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        std::thread::sleep(Duration::from_secs(1));
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();

        let stub_log = dir.path().join("amplihack.log");
        let stub = dir.path().join("amplihack");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo amplihack-test; exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                stub_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_BINARY_PATH", &stub);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "background");
        }

        let result = setup_blarify_indexing(&dirs).unwrap();

        let mut attempts = 0;
        while !stub_log.exists() && attempts < 20 {
            std::thread::sleep(Duration::from_millis(50));
            attempts += 1;
        }
        let logged = fs::read_to_string(&stub_log).unwrap();
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(result.indexing_active);
        assert!(
            result
                .status_context
                .as_deref()
                .is_some_and(|context| context.contains("Started background code-graph setup"))
        );
        assert!(logged.contains("index-code"));
        assert!(logged.contains(".amplihack/blarify.json"));
        assert!(logged.contains(".amplihack/graph_db"));
    }

    #[test]
    fn setup_blarify_indexing_sync_regenerates_stale_json_with_native_scip() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();
        let artifact_dir = dir.path().join(".amplihack");
        fs::create_dir_all(&artifact_dir).unwrap();
        fs::write(artifact_dir.join("blarify.json"), "{}\n").unwrap();
        std::thread::sleep(Duration::from_secs(1));
        fs::write(src_dir.join("app.py"), "print('updated')\n").unwrap();

        let stub_log = dir.path().join("amplihack-sync.log");
        let stub = dir.path().join("amplihack-sync");
        fs::write(
            &stub,
            format!(
                "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo amplihack-test; exit 0; fi\nprintf '%s\\n' \"$@\" > \"{}\"\n",
                stub_log.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_AMPLIHACK_BINARY_PATH", &stub);
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "sync");
        }

        let result = setup_blarify_indexing(&dirs).unwrap();
        let logged = fs::read_to_string(&stub_log).unwrap();

        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(!result.indexing_active);
        assert!(result.status_context.is_none());
        assert!(logged.contains("index-scip"));
        assert!(logged.contains("--project-path"));
        assert!(logged.contains(dir.path().to_string_lossy().as_ref()));
    }

    #[test]
    fn setup_blarify_indexing_reuses_existing_background_job() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        amplihack_cli::memory::record_background_index_pid(dir.path(), std::process::id()).unwrap();

        let result = setup_blarify_indexing(&dirs).unwrap();

        assert!(result.indexing_active);
        assert!(
            result
                .status_context
                .as_deref()
                .is_some_and(|context| context.contains("already running"))
        );
    }

    #[test]
    fn setup_blarify_indexing_skip_surfaces_status_notice() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let dirs = ProjectDirs::new(dir.path());
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/app.py"), "print('hi')\n").unwrap();
        unsafe {
            std::env::set_var("AMPLIHACK_BLARIFY_MODE", "skip");
        }

        let result = setup_blarify_indexing(&dirs).unwrap();

        unsafe {
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }

        assert!(!result.indexing_active);
        let context = result.status_context.expect("skip notice expected");
        assert!(context.contains("## Code Graph Status"));
        assert!(context.contains("AMPLIHACK_BLARIFY_MODE=skip"));
        assert!(context.contains("missing or stale"));
    }
}
