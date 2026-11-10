//! Durable, scoped warning journal. Lock order: caller's swap lock, then
//! this journal lock. Registration, publication and acknowledgement use the
//! same lock. Completion evidence belongs to the transaction owner; pending
//! intents alone never establish that credentials moved. Delivery is at least
//! once: acknowledge only after the delivery route reports success.
use crate::organisation_affinity::{Affinity, valid_id};
use crate::secure_files::{
    check_private_dir, check_private_file, ensure_private_dir, open_private_nofollow,
    read_bounded_nofollow, write_private_atomic,
};
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

const MAX_BYTES: u64 = 8 * 1024 * 1024;
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn name(value: &str) -> bool {
    valid_id(value) && value != "." && value != ".." && !value.contains(['/', '\\'])
}
fn check_ancestors(path: &Path) -> io::Result<()> {
    // A shared non-sticky parent could let another user replace an owned
    // root by renaming directories, even when the root itself is private.
    for parent in path.ancestors().skip(1) {
        let meta = fs::symlink_metadata(parent)?;
        // SAFETY: geteuid has no preconditions.
        let trusted_owner = meta.uid() == unsafe { libc::geteuid() } || meta.uid() == 0;
        let system_sticky = meta.uid() == 0 && meta.mode() & 0o1000 != 0;
        if !meta.is_dir()
            || meta.file_type().is_symlink()
            || !trusted_owner
            || (meta.mode() & 0o022 != 0 && !system_sticky)
        {
            return Err(invalid("warning root has an unsafe parent directory"));
        }
    }
    Ok(())
}
#[derive(Clone, Debug)]
pub struct Session {
    root: PathBuf,
    key: String,
}
#[derive(Clone, Debug)]
pub struct Intent {
    root: PathBuf,
    transaction_id: String,
}
#[derive(Clone, Copy, Debug)]
pub enum Completion {
    Committed,
    Aborted,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub version: u32,
    pub transaction_id: String,
    pub producer: String,
    pub reason: String,
    pub host: String,
    pub from_login: String,
    pub to_login: String,
    pub from_org: Option<String>,
    pub to_org: Option<String>,
    pub affinity: Affinity,
}
impl Event {
    fn validate(&self) -> io::Result<()> {
        if self.version != 1
            || !["rotator", "auto-switch"].contains(&self.producer.as_str())
            || ![
                "threshold",
                "upgrade",
                "plan-room",
                "failed-probe",
                "overflow-in",
                "overflow-out",
            ]
            .contains(&self.reason.as_str())
            || ![
                &self.transaction_id,
                &self.host,
                &self.from_login,
                &self.to_login,
            ]
            .into_iter()
            .all(|s| name(s))
            || ![&self.from_org, &self.to_org]
                .into_iter()
                .all(|o| o.as_deref().is_none_or(valid_id))
            || self.affinity != Affinity::between(self.from_org.as_deref(), self.to_org.as_deref())
        {
            return Err(invalid("invalid swap warning event"));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Warning {
    pub id: u64,
    #[serde(flatten)]
    pub event: Event,
    sessions: Vec<String>,
}
impl std::ops::Deref for Warning {
    type Target = Event;
    fn deref(&self) -> &Event {
        &self.event
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Registration {
    host: String,
    cursor: u64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Pending {
    event: Event,
    sessions: Vec<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    next: u64,
    sessions: BTreeMap<String, Registration>,
    pending: BTreeMap<String, Pending>,
    completed: BTreeMap<String, u64>,
    events: Vec<Warning>,
}
/// A journal bound to a canonical private profile root.
pub struct WarningHistory {
    root: PathBuf,
    dir: PathBuf,
}
struct Lock(fs::File);
impl Drop for Lock {
    fn drop(&mut self) {
        // SAFETY: the owned descriptor remains open.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl WarningHistory {
    pub fn open(root: &Path) -> io::Result<Self> {
        check_private_dir(root)?;
        let root = root.canonicalize()?;
        check_ancestors(&root)?;
        let dir = root.join(".swap-warning-history");
        ensure_private_dir(&dir)?;
        let this = Self { root, dir };
        this.access_read(|_| Ok(()))?;
        Ok(this)
    }
    fn access<T>(&self, operation: impl FnOnce(&mut Journal) -> io::Result<T>) -> io::Result<T> {
        self.with_state(true, operation)
    }
    fn access_read<T>(
        &self,
        operation: impl FnOnce(&mut Journal) -> io::Result<T>,
    ) -> io::Result<T> {
        self.with_state(false, operation)
    }
    fn with_state<T>(
        &self,
        persist: bool,
        operation: impl FnOnce(&mut Journal) -> io::Result<T>,
    ) -> io::Result<T> {
        check_private_dir(&self.root)?;
        check_ancestors(&self.root)?;
        check_private_dir(&self.dir)?;
        let file = open_private_nofollow(&self.dir.join("lock"))?;
        // Blocking on the kernel lock is event-driven; no retry timer.
        loop {
            // SAFETY: file owns a valid open descriptor.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } == 0 {
                break;
            }
            let e = io::Error::last_os_error();
            if e.kind() != io::ErrorKind::Interrupted {
                return Err(e);
            }
        }
        let _lock = Lock(file);
        let mut journal: Journal =
            match read_bounded_nofollow(&self.dir.join("journal.json"), MAX_BYTES)? {
                None => Journal::default(),
                Some((raw, meta)) => {
                    check_private_file(&meta)?;
                    serde_json::from_slice(&raw)
                        .map_err(|_| invalid("invalid warning journal encoding"))?
                }
            };
        let mut previous = 0;
        if journal
            .sessions
            .iter()
            .any(|(key, s)| !valid_id(key) || !name(&s.host) || s.cursor > journal.next)
            || journal.completed.iter().any(|(key, id)| {
                !name(key) || *id == 0 || *id > journal.next || journal.pending.contains_key(key)
            })
        {
            return Err(invalid("invalid warning journal identities or cursors"));
        }
        for warning in &journal.events {
            warning.event.validate()?;
            if warning.id <= previous || warning.id > journal.next {
                return Err(invalid("unordered warning history"));
            }
            previous = warning.id;
            if warning.sessions.iter().any(|key| {
                !journal
                    .sessions
                    .get(key)
                    .is_some_and(|s| s.host == warning.event.host)
            }) {
                return Err(invalid("warning scope is not registered"));
            }
        }
        for (key, pending) in &journal.pending {
            pending.event.validate()?;
            if key != &pending.event.transaction_id
                || pending.sessions.iter().any(|key| {
                    !journal
                        .sessions
                        .get(key)
                        .is_some_and(|s| s.host == pending.event.host)
                })
            {
                return Err(invalid("invalid pending warning scope or transaction"));
            }
        }
        let result = operation(&mut journal)?;
        if !persist {
            return Ok(result);
        }
        retire_dead_sessions(&mut journal, crate::session_warning_identity::birth);
        compact(&mut journal);
        let raw = serde_json::to_vec(&journal).map_err(io::Error::other)?;
        if raw.len() as u64 > MAX_BYTES {
            return Err(invalid("warning history full; unconsumed events retained"));
        }
        write_private_atomic(&self.dir, "journal.json", &raw)?;
        fs::File::open(&self.dir)?.sync_all()?;
        Ok(result)
    }
    /// Registers a process-bound lifecycle identity. Re-registration preserves
    /// its cursor. The opaque handle cannot address a different root.
    pub fn register_current_process(&mut self, session: &str, host: &str) -> io::Result<Session> {
        self.register_process(std::process::id(), session, host)
    }
    /// For lifecycle hooks: identity is bound to the kernel-reported parent,
    /// never to a caller-supplied PID.
    pub fn register_parent_process(&mut self, session: &str, host: &str) -> io::Result<Session> {
        // SAFETY: getppid has no preconditions.
        self.register_process(unsafe { libc::getppid() } as u32, session, host)
    }
    fn register_process(&mut self, pid: u32, session: &str, host: &str) -> io::Result<Session> {
        if session.len() > 256 || !name(session) || !name(host) {
            return Err(invalid("invalid session scope"));
        }
        let birth = crate::session_warning_identity::birth(pid)?;
        let key = format!("{pid}:{birth}:{session}");
        self.access(|j| {
            let r = j.sessions.entry(key.clone()).or_insert(Registration {
                host: host.into(),
                cursor: j.next,
            });
            if r.host != host {
                return Err(invalid("session host changed"));
            }
            Ok(())
        })?;
        Ok(Session {
            root: self.root.clone(),
            key,
        })
    }
    pub fn prepare(&mut self, value: &serde_json::Value) -> io::Result<Intent> {
        let event: Event = serde_json::from_value(value.clone())
            .map_err(|_| invalid("invalid swap warning event encoding"))?;
        event.validate()?;
        let tx = event.transaction_id.clone();
        self.access(|j| {
            if j.pending.contains_key(&tx) || j.completed.contains_key(&tx) {
                return Err(invalid("duplicate transaction identity"));
            }
            let sessions = j
                .sessions
                .iter()
                .filter(|(_, s)| s.host == event.host)
                .map(|(k, _)| k.clone())
                .collect();
            j.pending.insert(tx.clone(), Pending { event, sessions });
            Ok(())
        })?;
        Ok(Intent {
            root: self.root.clone(),
            transaction_id: tx,
        })
    }
    pub fn complete(&mut self, intent: &Intent, completion: Completion) -> io::Result<()> {
        if intent.root != self.root {
            return Err(invalid("intent belongs to another root"));
        }
        self.access(|j| {
            if j.completed.contains_key(&intent.transaction_id) {
                return Ok(());
            }
            if matches!(completion, Completion::Unknown) {
                return Err(invalid(
                    "swap completion unresolved; pending intent retained",
                ));
            }
            let pending = j
                .pending
                .remove(&intent.transaction_id)
                .ok_or_else(|| invalid("missing swap intent"))?;
            if matches!(completion, Completion::Aborted) {
                return Ok(());
            }
            j.next = j
                .next
                .checked_add(1)
                .ok_or_else(|| invalid("warning identity exhausted"))?;
            j.completed.insert(intent.transaction_id.clone(), j.next);
            // Registration and publication share this lock: a lifecycle
            // registered while a swap was in flight cannot fall between
            // the intent's scope snapshot and its completed event.
            let sessions = j
                .sessions
                .iter()
                .filter(|(_, s)| s.host == pending.event.host)
                .map(|(key, _)| key.clone())
                .collect();
            j.events.push(Warning {
                id: j.next,
                event: pending.event,
                sessions,
            });
            Ok(())
        })
    }

    /// Called by a transaction's verified completion/recovery path, before
    /// removing its authoritative transaction journal. Older transactions
    /// have no warning intent and return false.
    pub fn complete_transaction(
        &mut self,
        transaction: &str,
        producer: &str,
        host: &str,
        from: &str,
        to: &str,
    ) -> io::Result<bool> {
        self.settle_transaction(transaction, producer, host, from, to, Completion::Committed)
    }

    /// Discard an intent only after its producer has verified that the swap
    /// was rolled back. Unresolved transactions must retain their intents.
    pub fn abort_transaction(
        &mut self,
        transaction: &str,
        producer: &str,
        host: &str,
        from: &str,
        to: &str,
    ) -> io::Result<bool> {
        self.settle_transaction(transaction, producer, host, from, to, Completion::Aborted)
    }

    #[allow(clippy::too_many_arguments)]
    fn settle_transaction(
        &mut self,
        transaction: &str,
        producer: &str,
        host: &str,
        from: &str,
        to: &str,
        completion: Completion,
    ) -> io::Result<bool> {
        let present = self.access_read(|j| {
            if j.completed.contains_key(transaction) {
                if matches!(completion, Completion::Aborted) {
                    return Err(invalid("committed warning transaction cannot be aborted"));
                }
                return Ok(true);
            }
            let Some(pending) = j.pending.get(transaction) else {
                return Ok(false);
            };
            let event = &pending.event;
            if event.producer != producer
                || event.host != host
                || event.from_login != from
                || event.to_login != to
            {
                return Err(invalid(
                    "warning intent does not match completed transaction",
                ));
            }
            Ok(true)
        })?;
        if present {
            self.complete(
                &Intent {
                    root: self.root.clone(),
                    transaction_id: transaction.into(),
                },
                completion,
            )?;
        }
        Ok(present)
    }
    fn check(&self, session: &Session) -> io::Result<()> {
        if session.root != self.root {
            return Err(invalid("session belongs to another root"));
        }
        Ok(())
    }
    pub fn pending(&self, session: &Session) -> io::Result<Vec<Warning>> {
        self.check(session)?;
        self.access_read(|j| {
            let r = j
                .sessions
                .get(&session.key)
                .ok_or_else(|| invalid("unregistered session"))?;
            Ok(j.events
                .iter()
                .filter(|w| {
                    w.id > r.cursor
                        && w.sessions.contains(&session.key)
                        && w.affinity.warning().is_some()
                })
                .cloned()
                .collect())
        })
    }
    pub fn acknowledge(&mut self, session: &Session, id: u64) -> io::Result<()> {
        self.check(session)?;
        self.access(|j| {
            let cursor = j
                .sessions
                .get(&session.key)
                .ok_or_else(|| invalid("unregistered session"))?
                .cursor;
            if id <= cursor {
                return Ok(());
            }
            if j.events
                .iter()
                .find(|w| {
                    w.id > cursor
                        && w.sessions.contains(&session.key)
                        && w.affinity.warning().is_some()
                })
                .is_some_and(|w| w.id != id)
            {
                return Err(invalid("acknowledgement would skip an undelivered warning"));
            }
            if !j
                .events
                .iter()
                .any(|w| w.id == id && w.sessions.contains(&session.key))
            {
                return Err(invalid("warning not scoped to session"));
            }
            let r = j
                .sessions
                .get_mut(&session.key)
                .ok_or_else(|| invalid("unregistered session"))?;
            r.cursor = r.cursor.max(id);
            Ok(())
        })
    }
    /// Retire completion evidence only after the transaction owner confirms
    /// that no authoritative recovery journal can reference it. Pending intents
    /// and scoped warnings are unaffected.
    pub fn retire_completed_except(&mut self, referenced: &[String]) -> io::Result<()> {
        self.retire_completed_with(|| Ok(referenced.to_vec()))
    }
    /// Read authoritative references while holding the journal lock, so a
    /// concurrent publisher cannot appear between the reference snapshot and
    /// retirement. The callback must not acquire a swap or history lock.
    pub fn retire_completed_with(
        &mut self,
        references: impl FnOnce() -> io::Result<Vec<String>>,
    ) -> io::Result<()> {
        self.access(|j| {
            let referenced = references()?;
            j.completed.retain(|tx, _| referenced.contains(tx));
            Ok(())
        })
    }
    pub fn retain_required(&mut self) -> io::Result<()> {
        self.access(|j| {
            compact(j);
            Ok(())
        })
    }
    /// Warnings still required by at least one registered lifecycle.
    pub fn unconsumed(&self) -> io::Result<Vec<Warning>> {
        self.access_read(|j| {
            Ok(j.events
                .iter()
                .filter(|w| {
                    w.affinity.warning().is_some()
                        && w.sessions
                            .iter()
                            .any(|key| j.sessions.get(key).is_some_and(|s| s.cursor < w.id))
                })
                .cloned()
                .collect())
        })
    }
}

fn compact(j: &mut Journal) {
    j.events.retain(|w| {
        w.affinity.warning().is_some()
            && w.sessions
                .iter()
                .any(|key| j.sessions.get(key).is_some_and(|r| r.cursor < w.id))
    });
}

// A failed lookup is not generally evidence of death (permissions and platform
// failures must retain warnings). Pending transaction scopes remain untouched
// until their owner resolves the transaction.
fn retire_dead_sessions(j: &mut Journal, mut birth: impl FnMut(u32) -> io::Result<String>) {
    j.sessions.retain(|key, _| {
        if j.pending.values().any(|p| p.sessions.contains(key)) {
            return true;
        }
        let mut parts = key.splitn(3, ':');
        let Some(pid) = parts.next().and_then(|s| s.parse::<u32>().ok()) else {
            return true;
        };
        let Some(recorded) = parts.next() else {
            return true;
        };
        match birth(pid) {
            Ok(current) => current == recorded,
            Err(error) => !matches!(error.raw_os_error(), Some(libc::ESRCH) | Some(libc::ENOENT)),
        }
    });
    for event in &mut j.events {
        event.sessions.retain(|key| j.sessions.contains_key(key));
    }
}

#[cfg(test)]
mod retention_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn retirement_requires_kernel_evidence_and_preserves_pending_scopes() {
        let mut journal = Journal::default();
        for pid in 1..=6 {
            journal.sessions.insert(
                format!("{pid}:original:s"),
                Registration {
                    host: "home".into(),
                    cursor: 0,
                },
            );
        }
        let event = Event {
            version: 1,
            transaction_id: "tx".into(),
            producer: "rotator".into(),
            reason: "threshold".into(),
            host: "home".into(),
            from_login: "a".into(),
            to_login: "b".into(),
            from_org: Some("a".into()),
            to_org: Some("b".into()),
            affinity: Affinity::Crossing,
        };
        journal.pending.insert(
            "tx".into(),
            Pending {
                event: event.clone(),
                sessions: vec!["5:original:s".into()],
            },
        );
        journal.events.push(Warning {
            id: 1,
            event,
            sessions: journal.sessions.keys().cloned().collect(),
        });
        retire_dead_sessions(&mut journal, |pid| match pid {
            1 => Ok("original".into()),
            2 => Ok("reused".into()),
            3 | 5 => Err(io::Error::from_raw_os_error(libc::ESRCH)),
            6 => Err(io::Error::from_raw_os_error(libc::EPERM)),
            _ => Err(io::Error::from_raw_os_error(libc::EACCES)),
        });
        assert_eq!(
            journal.sessions.keys().cloned().collect::<Vec<_>>(),
            [
                "1:original:s",
                "4:original:s",
                "5:original:s",
                "6:original:s"
            ]
        );
        assert_eq!(journal.events[0].sessions.len(), 4);
        assert_eq!(journal.pending["tx"].sessions, ["5:original:s"]);
        // Resolving the transaction releases its scope for safe retirement.
        journal.pending.clear();
        retire_dead_sessions(&mut journal, |_| {
            Err(io::Error::from_raw_os_error(libc::ENOENT))
        });
        compact(&mut journal);
        assert!(journal.sessions.is_empty());
        assert!(journal.events.is_empty());
    }

    #[test]
    fn full_live_state_fails_without_overwriting_durable_state() {
        let tmp = tempfile::tempdir().unwrap();
        fs::set_permissions(tmp.path(), fs::Permissions::from_mode(0o700)).unwrap();
        let h = WarningHistory::open(tmp.path()).unwrap();
        h.access(|_| Ok(())).unwrap();
        let path = tmp.path().join(".swap-warning-history/journal.json");
        let before = fs::read(&path).unwrap();
        let error = h
            .access(|j| {
                for n in 0..40_000 {
                    let tx = format!("tx-{n}");
                    j.pending.insert(
                        tx.clone(),
                        Pending {
                            event: Event {
                                version: 1,
                                transaction_id: tx,
                                producer: "rotator".into(),
                                reason: "threshold".into(),
                                host: "home".into(),
                                from_login: "work".into(),
                                to_login: "spare".into(),
                                from_org: Some("a".into()),
                                to_org: Some("b".into()),
                                affinity: Affinity::between(Some("a"), Some("b")),
                            },
                            sessions: vec![],
                        },
                    );
                }
                Ok(())
            })
            .unwrap_err();
        assert!(error.to_string().contains("history full"));
        assert_eq!(fs::read(path).unwrap(), before);
    }
}
