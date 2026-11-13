//! Dependency scheduling driven by child exits and cancellation signals.
use super::{execution, models::Workstream, persistence::persist_state, waves};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs,
    process::Child,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Instant,
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

fn cancel(children: &mut HashMap<usize, Child>, leaves: &mut [Workstream]) -> Result<()> {
    for (&index, child) in children.iter_mut() {
        #[cfg(unix)]
        {
            let result = unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            if result != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        #[cfg(not(unix))]
        {
            child.kill()?;
        }
        child.wait()?;
        let ws = &mut leaves[index];
        ws.lifecycle_state = "interrupted_resumable".into();
        ws.cleanup_eligible = false;
        ws.pid = None;
        ws.exit_code = None;
        persist_state(ws)?;
    }
    children.clear();
    Ok(())
}

pub(super) fn execute(
    leaves: &mut [Workstream],
    mode: &str,
    running: Arc<AtomicBool>,
) -> Result<()> {
    let (tx, rx) = mpsc::channel();
    let _signals = signals(tx.clone())?;
    let limit = waves::wave_size(leaves.len(), waves::concurrency_limit());
    let mut children = HashMap::new();
    let result = (|| -> Result<()> {
        loop {
            ensure!(
                running.load(Ordering::Relaxed),
                "workstream execution cancelled"
            );
            for index in 0..leaves.len() {
                if leaves[index].lifecycle_state == "completed"
                    || children.contains_key(&index)
                    || ["blocked", "partial", "failed_terminal"]
                        .contains(&leaves[index].lifecycle_state.as_str())
                {
                    continue;
                }
                let mut prerequisites = serde_json::Map::new();
                let mut ready = true;
                let mut failed = false;
                for dep in &leaves[index].dependencies {
                    let other = leaves
                        .iter()
                        .find(|leaf| leaf.key.as_str() == dep)
                        .context("validated dependency disappeared")?;
                    if other.lifecycle_state == "completed" {
                        prerequisites.insert(dep.clone(), other.result.clone());
                    } else {
                        ready = false;
                        failed |= ["blocked", "partial", "failed_terminal"]
                            .contains(&other.lifecycle_state.as_str());
                    }
                }
                if failed {
                    leaves[index].lifecycle_state = "blocked".into();
                    persist_state(&leaves[index])?;
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
            match rx.recv().context("completion event channel closed")? {
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
    if result.is_err() {
        cancel(&mut children, leaves)?;
    }
    result
}
