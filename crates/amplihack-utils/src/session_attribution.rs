//! Session identity is captured independently of status-line and hook claims.
//! Current credential location cannot relabel an already-running session.
use crate::usage_observation::Binding;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct SessionBinding {
    pub session_id: String,
    pub binding: Binding,
}
#[derive(Debug, Clone)]
pub struct LeaseBinding {
    pub session_id: String,
    pub host_profile: String,
    pub binding: Binding,
}
#[derive(Debug, Clone, Default)]
pub struct AttributionSnapshot {
    pub login_bindings: Vec<Binding>,
    pub sessions: Vec<SessionBinding>,
    pub leases: Vec<LeaseBinding>,
    pub live: Option<Binding>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttributionError {
    MissingContext,
    Ambiguous,
    RetiredGeneration,
    InvalidClaim,
}

pub fn resolve(
    payload: &Value,
    trusted: &AttributionSnapshot,
) -> Result<Binding, AttributionError> {
    let session = payload
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or(AttributionError::MissingContext)?;
    let mut candidates = trusted
        .sessions
        .iter()
        .filter(|s| s.session_id == session)
        .map(|s| &s.binding)
        .chain(
            trusted
                .leases
                .iter()
                .filter(|s| s.session_id == session)
                .map(|s| &s.binding),
        );
    let binding = candidates.next().ok_or(AttributionError::MissingContext)?;
    if candidates.any(|b| b != binding) {
        return Err(AttributionError::Ambiguous);
    }
    if !trusted.login_bindings.contains(binding) {
        return Err(AttributionError::RetiredGeneration);
    }
    for (key, expected) in [
        ("profile", &binding.profile),
        ("account_id", &binding.account_id),
        ("generation", &binding.generation),
    ] {
        if let Some(claim) = payload.get(key)
            && claim.as_str() != Some(expected)
        {
            return Err(AttributionError::InvalidClaim);
        }
    }
    Ok(binding.clone())
}

/// Capture immutable ownership and ordering before interpreting a host's
/// usage windows. Missing or retired session context is display-only.
pub fn capture(
    root: &std::path::Path,
    payload: &Value,
    now: i64,
) -> std::io::Result<Option<crate::usage_observation::Capture>> {
    let store = crate::usage_observation::ObservationStore::open(root)?;
    let mut guard = store.lock()?;
    let snapshot = AttributionSnapshot {
        login_bindings: guard.bindings()?,
        sessions: guard.sessions()?,
        ..Default::default()
    };
    let binding = match resolve(payload, &snapshot) {
        Ok(binding) => binding,
        Err(_) => return Ok(None),
    };
    Ok(Some(guard.capture_new(&binding, now)?))
}

/// Publish only through independently persisted session context. Unattributed
/// payloads remain display-only; they cannot establish or repair a login.
pub fn publish(
    root: &std::path::Path,
    payload: &Value,
    evidence: &crate::usage_observation::Evidence,
    now: i64,
) -> std::io::Result<crate::usage_observation::PublishOutcome> {
    use crate::usage_observation::{ObservationStore, PublishOutcome};
    let store = ObservationStore::open(root)?;
    let mut guard = store.lock()?;
    let snapshot = AttributionSnapshot {
        login_bindings: guard.bindings()?,
        sessions: guard.sessions()?,
        ..AttributionSnapshot::default()
    };
    let binding = match resolve(payload, &snapshot) {
        Ok(binding) => binding,
        Err(AttributionError::RetiredGeneration) => return Ok(PublishOutcome::RetiredGeneration),
        Err(_) => return Ok(PublishOutcome::InvalidAttribution),
    };
    // Each accepted host invocation is a new capture. Duplicate kernel
    // notifications and identical evidence are suppressed by the coordinator.
    let capture = guard.capture_new(&binding, now)?;
    guard.publish(&capture, evidence)
}

/// Extract measured windows without display substitutions. A missing reset
/// or percentage stays missing. Invalid numbers are rejected by the store.
pub fn status_evidence(payload: &Value) -> std::io::Result<crate::usage_observation::Evidence> {
    let window = |key: &str| payload.get("rate_limits").and_then(|r| r.get(key));
    let invalid = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "usage window contains an invalid percentage or reset timestamp",
        )
    };
    let mut values = Vec::new();
    for key in ["five_hour", "seven_day"] {
        let Some(value) = window(key).filter(|v| !v.is_null()) else {
            values.push((None, None));
            continue;
        };
        let object = value.as_object().ok_or_else(invalid)?;
        let percentage = match object.get("used_percentage") {
            None | Some(Value::Null) => None,
            Some(v) => Some(v.as_f64().ok_or_else(invalid)?),
        };
        let reset = match object.get("resets_at") {
            None | Some(Value::Null) => None,
            Some(v) => Some(
                v.as_i64()
                    .or_else(|| {
                        v.as_str()
                            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                            .map(|d| d.timestamp())
                    })
                    .ok_or_else(invalid)?,
            ),
        };
        values.push((percentage, reset));
    }
    Ok(crate::usage_observation::Evidence {
        session_window: match window("five_hour") {
            Some(Value::Null) => crate::usage_observation::SessionWindow::Absent,
            Some(Value::Object(_)) => crate::usage_observation::SessionWindow::Present,
            _ => crate::usage_observation::SessionWindow::Unknown,
        },
        session_used: values[0].0,
        week_used: values[1].0,
        session_reset_at: values[0].1,
        week_reset_at: values[1].1,
        limit_reached: false,
    })
}

/// SessionStart captures once. A session already bound to an earlier login
/// remains bound to it; current credential location cannot redirect it.
pub fn capture_start(root: &std::path::Path, session: &str, _now: i64) -> std::io::Result<()> {
    use crate::usage_observation::ObservationStore;
    let store = ObservationStore::open(root)?;
    let mut guard = store.lock()?;
    if guard.sessions()?.iter().any(|s| s.session_id == session) {
        return Ok(());
    }
    let home = crate::rotator_status::home_from_env();
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .or_else(|| home.as_ref().map(|h| h.join(".claude")));
    let Some(config) = config else {
        return Ok(());
    };
    let config = config.canonicalize()?;
    let root = root.canonicalize()?;
    let is_home = home
        .and_then(|h| h.join(".claude").canonicalize().ok())
        .as_ref()
        == Some(&config);
    if config.parent() != Some(root.as_path()) && !is_home {
        return Ok(());
    }
    if let Some(binding) = guard.binding_at(&config.join(".credentials.json"))? {
        guard.bind_session(session, &binding)?;
    }
    Ok(())
}
