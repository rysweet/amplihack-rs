//! Dependency scheduling driven by child exits and cancellation signals.
use super::{execution, models::Workstream, persistence::persist_state, waves};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    process::Child,
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

enum Event {
    Exited(usize, std::io::Result<()>),
    Cancel,
}

struct Signals {
    handle: signal_hook::iterator::Handle,
    thread: Option<thread::JoinHandle<()>>,
}
impl Drop for Signals {
    fn drop(&mut self) {
        self.handle.close();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn signals(tx: mpsc::Sender<Event>) -> Result<Signals> {
    let mut signals = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGINT,
        signal_hook::consts::SIGTERM,
    ])?;
    let handle = signals.handle();
    let thread = thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = tx.send(Event::Cancel);
        }
    });
    Ok(Signals {
        handle,
        thread: Some(thread),
    })
}

// WNOWAIT leaves the PID reserved until the owning scheduler reaps it. This
// prevents a cancellation signal from targeting a recycled PID after an exit.
#[cfg(unix)]
fn observe(pid: u32) -> std::io::Result<()> {
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    loop {
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            )
        };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

fn complete(ws: &mut Workstream, code: i32) -> Result<()> {
    ws.pid = None;
    if let Some(capture) = ws.output_capture.take() {
        capture.finish()?;
    }
    if ws.lifecycle_state == "timed_out_resumable" && ws.timeout_policy == "interrupt-preserve" {
        ws.exit_code = Some(code);
        ws.end_time = Some(Instant::now());
        ws.cleanup_eligible = false;
        return persist_state(ws);
    }
    let progress = match fs::read(&ws.progress_file) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).context("invalid progress sidecar")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(e.into()),
    };
    let output = fs::read_to_string(ws.artefact_dir.join("output.json"))?;
    let runner = if output.trim_start().starts_with('{') {
        serde_json::from_str::<Value>(&output).context("invalid runner result JSON")?
    } else {
        Value::Null
    };
    let step_output = runner
        .get("step_results")
        .and_then(Value::as_array)
        .and_then(|steps| {
            steps
                .iter()
                .find(|step| step["step_id"] == "execute-operation")
        })
        .and_then(|step| step.get("output"))
        .and_then(Value::as_str);
    let result = progress
        .get("operations_result")
        .and_then(Value::as_str)
        .or_else(|| {
            step_output.and_then(|text| {
                text.lines()
                    .rev()
                    .map(str::trim)
                    .find(|line| line.starts_with("OPERATIONS_RESULT:"))
            })
        })
        .or_else(|| {
            output
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| line.starts_with("OPERATIONS_RESULT:"))
        });
    let code = if runner.get("success") == Some(&json!(false)) {
        -1
    } else {
        code
    };
    ws.result = if runner.is_object() {
        runner.clone()
    } else {
        progress.clone()
    };
    if let Some(result) = result {
        if !ws.result.is_object() {
            ws.result = json!({});
        }
        ws.result["operations_result"] = json!(result);
    }
    ws.lifecycle_state = if code != 0 {
        "failed_terminal"
    } else if ws.classification == "Operations" {
        match result
            .and_then(|r| r.strip_prefix("OPERATIONS_RESULT:"))
            .map(str::trim_start)
        {
            Some(r) if r.starts_with("completed — ") => "completed",
            Some(r) if r.starts_with("partial — ") => "partial",
            _ => "failed_terminal",
        }
    } else {
        "completed"
    }
    .into();
    ws.exit_code = Some(code);
    ws.pid = None;
    ws.end_time = Some(Instant::now());
    ws.cleanup_eligible = false;
    persist_state(ws)
}

fn stop(child: &mut Child) -> Result<()> {
    #[cfg(unix)]
    {
        let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        if result != 0 && std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    #[cfg(not(unix))]
    child.kill()?;
    Ok(())
}

fn cancel(children: &mut HashMap<usize, Child>, leaves: &mut [Workstream]) -> Result<()> {
    let mut failure = None;
    for (index, mut child) in children.drain() {
        let result = (|| -> Result<()> {
            stop(&mut child)?;
            child.wait()?;
            let ws = &mut leaves[index];
            if let Some(capture) = ws.output_capture.take() {
                capture.finish()?;
            }
            ws.lifecycle_state = "interrupted_resumable".into();
            ws.cleanup_eligible = false;
            ws.pid = None;
            ws.exit_code = None;
            persist_state(ws)
        })();
        if let Err(error) = result {
            failure.get_or_insert(error);
        }
    }
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

/// Reach a fixed point before scheduling or terminating, irrespective of plan order.
fn block_dependents(leaves: &mut [Workstream], children: &HashMap<usize, Child>) -> Result<()> {
    loop {
        let mut changed = false;
        for index in 0..leaves.len() {
            if children.contains_key(&index) || leaves[index].lifecycle_state != "pending" {
                continue;
            }
            let blocked = leaves[index].dependencies.iter().any(|dep| {
                leaves.iter().enumerate().any(|(i, ws)| {
                    ws.key.as_str() == dep
                        && !children.contains_key(&i)
                        && [
                            "blocked",
                            "partial",
                            "failed_terminal",
                            "failed_resumable",
                            "interrupted_resumable",
                            "timed_out_resumable",
                            "abandoned",
                        ]
                        .contains(&ws.lifecycle_state.as_str())
                })
            });
            if blocked {
                leaves[index].lifecycle_state = "blocked".into();
                persist_state(&leaves[index])?;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
    }
}

/// Completion remains event-driven; only configured deadlines wake the receiver.
fn timeout_wait(
    leaves: &mut [Workstream],
    children: &mut HashMap<usize, Child>,
) -> Result<Option<Duration>> {
    let mut wait = None;
    for (&index, child) in children {
        let ws = &mut leaves[index];
        let remaining = Duration::from_secs(ws.max_runtime).saturating_sub(
            ws.start_time
                .context("owned child has no start time")?
                .elapsed(),
        );
        let next = if !remaining.is_zero() {
            Some(remaining)
        } else {
            if ws.lifecycle_state == "running" {
                ws.lifecycle_state = "timed_out_resumable".into();
                ws.cleanup_eligible = false;
                persist_state(ws)?;
            }
            if ws.timeout_policy == "interrupt-preserve" && ws.exit_code.is_none() {
                let idle = ws
                    .output_capture
                    .as_ref()
                    .context("owned child has no output observer")?
                    .idle_remaining();
                if idle.is_zero() {
                    stop(child)?;
                    // The observer reports exit; the scheduler reaps exactly once.
                    ws.exit_code = Some(-1);
                    None
                } else {
                    Some(idle)
                }
            } else {
                None
            }
        };
        if let Some(next) = next {
            wait = Some(wait.map_or(next, |old: Duration| old.min(next)));
        }
    }
    Ok(wait)
}

pub(super) fn execute(leaves: &mut [Workstream], mode: &str) -> Result<()> {
    // Only entry-time recovery may retry a reaped resumable execution. A
    // timeout produced in this run must remain terminal for its dependents.
    for ws in leaves.iter_mut() {
        if [
            "interrupted_resumable",
            "timed_out_resumable",
            "failed_resumable",
            "tracking",
            "blocked",
        ]
        .contains(&ws.lifecycle_state.as_str())
        {
            ws.lifecycle_state = "pending".into();
        }
    }
    let (tx, rx) = mpsc::channel();
    let _signals = signals(tx.clone())?;
    let limit = waves::wave_size(leaves.len(), waves::concurrency_limit());
    let mut children = HashMap::new();
    let result = (|| -> Result<()> {
        loop {
            block_dependents(leaves, &children)?;
            for index in 0..leaves.len() {
                if leaves[index].lifecycle_state != "pending" || children.contains_key(&index) {
                    continue;
                }
                let mut prerequisites = serde_json::Map::new();
                let mut ready = true;
                for dep in &leaves[index].dependencies {
                    let other = leaves
                        .iter()
                        .find(|leaf| leaf.key.as_str() == dep)
                        .context("validated dependency disappeared")?;
                    if other.lifecycle_state == "completed" {
                        prerequisites.insert(dep.clone(), other.result.clone());
                    } else {
                        ready = false;
                    }
                }
                if !ready || children.len() >= limit {
                    continue;
                }
                let child =
                    execution::launch(&mut leaves[index], mode, Value::Object(prerequisites))?;
                let pid = child.id();
                children.insert(index, child);
                let tx = tx.clone();
                thread::spawn(move || {
                    let _ = tx.send(Event::Exited(index, observe(pid)));
                });
            }
            if children.is_empty() {
                break;
            }
            // Prefer queued completion/cancellation over a deadline check.
            let event = match rx.try_recv() {
                Ok(event) => event,
                Err(mpsc::TryRecvError::Disconnected) => {
                    anyhow::bail!("completion event channel closed")
                }
                Err(mpsc::TryRecvError::Empty) => match timeout_wait(leaves, &mut children)? {
                    Some(duration) => match rx.recv_timeout(duration) {
                        Ok(event) => event,
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        Err(error) => return Err(error.into()),
                    },
                    None => rx.recv().context("completion event channel closed")?,
                },
            };
            match event {
                Event::Exited(index, status) => {
                    status.context("observe owned child exit")?;
                    let mut child = children
                        .remove(&index)
                        .context("unexpected completion event")?;
                    complete(&mut leaves[index], child.wait()?.code().unwrap_or(-1))?;
                }
                Event::Cancel => {
                    anyhow::bail!("workstream execution cancelled; recovery state retained")
                }
            }
        }
        ensure!(
            leaves.iter().all(|ws| ws.lifecycle_state == "completed"),
            "one or more workstreams failed, were partial or are blocked; see persisted state"
        );
        Ok(())
    })();
    if let Err(error) = result {
        if let Err(cleanup) = cancel(&mut children, leaves) {
            return Err(error.context(format!("cleanup also failed: {cleanup:#}")));
        }
        if let Err(blocking) = block_dependents(leaves, &children) {
            return Err(error.context(format!(
                "saving blocked dependents also failed: {blocking:#}"
            )));
        }
        return Err(error);
    }
    Ok(())
}
