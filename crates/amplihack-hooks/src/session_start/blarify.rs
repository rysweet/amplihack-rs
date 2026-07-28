//! Blarify code-graph setup and indexing.

use amplihack_memory::cli_memory::{
    background_index_job_active, check_index_status, resolve_code_graph_db_path_for_project,
};
use amplihack_types::ProjectDirs;
use amplihack_utils::binary_finder::BinaryFinder;
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
        amplihack_memory::cli_memory::record_background_index_pid(project_root, child.id())?;
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
    status: &amplihack_memory::cli_memory::IndexStatus,
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
    status: &amplihack_memory::cli_memory::IndexStatus,
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
    use std::path::Path;
    use std::time::Duration;

    /// Terminal-or-continue decision for one `poll_file_for_content` iteration.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LivenessStep {
        /// All markers observed -- stop and report success.
        Found,
        /// File has been idle (no growth) for at least `idle_bound` without all
        /// markers -- stop and report a give-up (the real loop returns
        /// `(false, content)`).
        GiveUp,
        /// Alive: markers not yet complete but growth is still resetting the
        /// idle timer, or the idle bound has not elapsed -- keep polling.
        Alive,
    }

    /// Pure liveness decision. Contains no clock, no I/O, no sleep, so it is
    /// fully deterministic and cannot race under load.
    ///
    /// * `grew` -- did the file's byte length change since the previous poll?
    ///   Growth resets the idle timer: whenever `grew` is `true` this function
    ///   returns `Alive` (unless the markers are already complete), regardless
    ///   of `idle_elapsed`. The give-up branch is gated by the explicit `!grew`
    ///   guard, so a growing file is never truncated even if `idle_elapsed` is
    ///   large.
    /// * `all_markers_present` -- are all required markers in the content read?
    /// * `idle_elapsed` -- time since the last observed growth. The real loop
    ///   passes `last_progress.elapsed()` here; on a growth observation this
    ///   value is neutralized by the `!grew` guard, not by being zero.
    /// * `idle_bound` -- how long the file may stay idle before we give up.
    fn liveness_step(
        grew: bool,
        all_markers_present: bool,
        idle_elapsed: Duration,
        idle_bound: Duration,
    ) -> LivenessStep {
        if all_markers_present {
            LivenessStep::Found
        } else if !grew && idle_elapsed >= idle_bound {
            LivenessStep::GiveUp
        } else {
            LivenessStep::Alive
        }
    }

    /// Adaptive, liveness-based content poll (issue #908).
    ///
    /// The background subprocess writes its stub log via a shell redirect
    /// (`printf ... > log`), which creates the file *before* its content is
    /// flushed. Polling for mere file existence therefore races the write under
    /// CI load. Instead we poll the file *content* until all `markers` appear.
    ///
    /// Liveness rather than a fixed cap: any growth in the file's byte length
    /// resets an idle timer, so a slow-but-alive subprocess is never truncated.
    /// We give up only once the file has been idle (no growth) for `idle_bound`.
    ///
    /// Returns `(all_markers_found, last_content_read)`.
    fn poll_file_for_content(
        path: &Path,
        markers: &[&str],
        poll_interval: Duration,
        idle_bound: Duration,
    ) -> (bool, String) {
        let mut last_len: u64 = 0;
        let mut last_progress = std::time::Instant::now();
        loop {
            let current_len = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            let grew = current_len != last_len;
            if grew {
                last_len = current_len;
                last_progress = std::time::Instant::now();
            }
            let content = fs::read_to_string(path).unwrap_or_default();
            let all_markers_present = markers.iter().all(|m| content.contains(m));

            match liveness_step(
                grew,
                all_markers_present,
                last_progress.elapsed(),
                idle_bound,
            ) {
                LivenessStep::Found => return (true, content),
                LivenessStep::GiveUp => return (false, content),
                LivenessStep::Alive => std::thread::sleep(poll_interval),
            }
        }
    }

    #[test]
    fn content_present_immediately_returns_found_fast() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("ready.log");
        fs::write(
            &log,
            "index-code .amplihack/blarify.json .amplihack/graph_db\n",
        )
        .unwrap();

        let start = std::time::Instant::now();
        let (found, content) = poll_file_for_content(
            &log,
            &[
                "index-code",
                ".amplihack/blarify.json",
                ".amplihack/graph_db",
            ],
            Duration::from_millis(25),
            Duration::from_secs(2),
        );

        assert!(found);
        assert!(content.contains("index-code"));
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "already-present content should return without waiting the idle bound"
        );
    }

    #[test]
    fn content_appended_after_delay_is_detected_via_liveness() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("delayed.log");
        fs::write(&log, "starting\n").unwrap();

        let log_writer = log.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            fs::write(&log_writer, "index-code done\n").unwrap();
        });

        let (found, content) = poll_file_for_content(
            &log,
            &["index-code"],
            Duration::from_millis(25),
            Duration::from_secs(2),
        );
        writer.join().unwrap();

        assert!(found);
        assert!(content.contains("index-code"));
    }

    #[test]
    fn file_idle_without_markers_gives_up_after_idle_bound() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("idle.log");
        fs::write(&log, "partial-output-only\n").unwrap();

        let idle_bound = Duration::from_millis(300);
        let start = std::time::Instant::now();
        let (found, _content) =
            poll_file_for_content(&log, &["index-code"], Duration::from_millis(25), idle_bound);

        assert!(!found, "should give up when markers never appear");
        assert!(
            start.elapsed() >= idle_bound,
            "must wait at least the idle bound before giving up"
        );
        assert!(
            start.elapsed() < idle_bound + Duration::from_secs(2),
            "idle give-up must be bounded, not indefinite"
        );
    }

    #[test]
    fn slow_growing_file_is_not_truncated_before_markers_arrive() {
        // Deterministic replacement for the old thread+sleep race. We script
        // the exact sequence of observations the real loop would make for a
        // slow-but-alive subprocess and assert the decision at each step. No
        // clock, thread, or filesystem is involved, so CI load cannot perturb
        // the result.
        //
        // Each slow write is observed as growth only *after* the idle window
        // would otherwise have expired (`idle_elapsed >= idle_bound`). That is
        // precisely the #1093 race: growth must still override the elapsed idle
        // bound and keep the poll alive, so we feed `idle_bound` -- not zero --
        // to prove the `!grew` guard, not merely a reset timer, does the work.
        let idle_bound = Duration::from_millis(200);

        // Four slow-but-live growth observations before the marker arrives.
        for _ in 0..4 {
            assert_eq!(
                liveness_step(true, false, idle_bound, idle_bound),
                LivenessStep::Alive,
                "growth must override an elapsed idle bound and keep the poll alive"
            );
        }

        // Final observation: the last write brought in `index-code`.
        assert_eq!(
            liveness_step(true, true, idle_bound, idle_bound),
            LivenessStep::Found,
            "markers present on a live file must report Found, never truncate"
        );
    }

    #[test]
    fn liveness_step_gives_up_only_when_idle_past_bound() {
        let bound = Duration::from_millis(200);

        // Idle but not yet past the bound -> keep polling.
        assert_eq!(
            liveness_step(false, false, Duration::from_millis(199), bound),
            LivenessStep::Alive,
            "idle below the bound must keep polling"
        );
        // Idle at/over the bound with no markers -> give up.
        assert_eq!(
            liveness_step(false, false, bound, bound),
            LivenessStep::GiveUp,
            "idle at the bound with no markers must give up"
        );
        // A growing file is never truncated, even if idle_elapsed is large.
        assert_eq!(
            liveness_step(true, false, bound, bound),
            LivenessStep::Alive,
            "growth must override the idle bound and keep polling"
        );
        // Markers present wins even if idle past the bound.
        assert_eq!(
            liveness_step(false, true, bound, bound),
            LivenessStep::Found,
            "markers present must report Found regardless of idle time"
        );
    }

    #[test]
    fn missing_file_then_created_is_handled() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("late.log");
        // Note: file intentionally absent at poll start.

        let log_writer = log.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            fs::write(&log_writer, "index-code appears\n").unwrap();
        });

        let (found, content) = poll_file_for_content(
            &log,
            &["index-code"],
            Duration::from_millis(25),
            Duration::from_secs(2),
        );
        writer.join().unwrap();

        assert!(
            found,
            "a file created after polling begins must still be detected"
        );
        assert!(content.contains("index-code"));
    }

    #[test]
    fn setup_blarify_indexing_background_imports_current_json_when_db_missing() {
        let _guard = env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let saved_graph_db = std::env::var_os("AMPLIHACK_GRAPH_DB_PATH");
        unsafe { std::env::remove_var("AMPLIHACK_GRAPH_DB_PATH") };

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

        // Adaptive, liveness-based content poll (see `poll_file_for_content`).
        // The background subprocess writes the stub log via a shell redirect
        // (`printf ... > log`), which creates the file before its content is
        // flushed. Polling for mere existence races the write under CI load, so
        // we poll the *content* and keep waiting as long as the log is still
        // growing, giving up only after a bounded idle interval.
        let (_found, logged) = poll_file_for_content(
            &stub_log,
            &[
                "index-code",
                ".amplihack/blarify.json",
                ".amplihack/graph_db",
            ],
            Duration::from_millis(25),
            Duration::from_secs(2),
        );
        unsafe {
            std::env::remove_var("AMPLIHACK_AMPLIHACK_BINARY_PATH");
            std::env::remove_var("AMPLIHACK_BLARIFY_MODE");
        }
        if let Some(val) = saved_graph_db {
            unsafe { std::env::set_var("AMPLIHACK_GRAPH_DB_PATH", val) };
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
        amplihack_memory::cli_memory::record_background_index_pid(dir.path(), std::process::id())
            .unwrap();

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
