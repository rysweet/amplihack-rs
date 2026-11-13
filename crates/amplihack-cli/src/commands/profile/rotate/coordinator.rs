//! Owns logical decisions and their generation-bound jobs. Time is supplied
//! only for policy evaluation; it can never make work ready.
use super::settings::Settings;
use crate::commands::profile::ProfileError;
use std::collections::{BTreeMap, BTreeSet};

pub(super) use super::coordinator_types::*;
struct Decision {
    inputs: InputState,
    jobs: Vec<ProbeJob>,
    attempted: BTreeSet<(String, String)>,
    rotate: bool,
    candidates: bool,
    failed: bool,
}
pub(super) struct Coordinator {
    settings: Settings,
    previous: Option<InputState>,
    next: u64,
    consumed: Option<InputState>,
    namespace: String,
    active: BTreeMap<u64, Decision>,
}
impl InputState {
    fn observation(&self, login: &LoginInput) -> Option<&EvidenceInput> {
        self.observations
            .iter()
            .filter(|e| {
                e.profile == login.profile
                    && e.account_id == login.account_id
                    && e.generation == login.generation
            })
            .max_by_key(|e| e.ticket)
    }
    fn candidate(&self, login: &LoginInput) -> bool {
        login.eligible
            && !login.live
            && self
                .logins
                .iter()
                .find(|l| l.live)
                .and_then(|l| l.organisation.as_ref())
                .is_some_and(|org| login.organisation.as_ref() == Some(org))
    }
    fn same_policy(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.held == other.held
            && self.control == other.control
            && self.logins == other.logins
            && self
                .logins
                .iter()
                .all(|l| same_evidence(self.observation(l), other.observation(l)))
    }
}
fn same_evidence(a: Option<&EvidenceInput>, b: Option<&EvidenceInput>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            a.session_window == b.session_window
                && a.session_reset_at == b.session_reset_at
                && a.week_reset_at == b.week_reset_at
                && a.session_used == b.session_used
                && a.week_used == b.week_used
                && a.limit_reached == b.limit_reached
        }
        (None, None) => true,
        _ => false,
    }
}
impl Coordinator {
    pub fn new(settings: Settings) -> Self {
        Self {
            settings,
            previous: None,
            next: 0,
            consumed: None,
            namespace: String::new(),
            active: BTreeMap::new(),
        }
    }
    fn validate(state: &InputState) -> Result<(), ProfileError> {
        if state.logins.len() > 4096 || state.observations.len() > 4096 {
            return Err(ProfileError::RotateDaemon(
                "event input capacity exceeded".into(),
            ));
        }
        Ok(())
    }
    pub fn baseline(&mut self, state: &InputState, _now: i64) -> Result<Vec<Effect>, ProfileError> {
        Self::validate(state)?;
        if self.namespace.is_empty() {
            use std::io::Read;
            let mut bytes = [0u8; 16];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
            self.namespace = format!("{:032x}", u128::from_ne_bytes(bytes));
        }
        self.previous = Some(state.clone());
        Ok(Vec::new())
    }
    /// Draining readiness never runs policy. In particular advancing a fake
    /// clock cannot retry a failure, rescan inputs, expire a pin or archive.
    pub fn ready(&mut self, _state: &InputState, _now: i64) -> Result<Vec<Effect>, ProfileError> {
        Ok(Vec::new())
    }
    fn needs_relief(&self, state: &InputState) -> bool {
        state
            .logins
            .iter()
            .filter(|l| l.live && l.eligible)
            .any(|l| {
                state.observation(l).is_some_and(|e| {
                    e.limit_reached
                        || e.session_used
                            .is_some_and(|v| v >= f64::from(self.settings.session_at))
                        || e.week_used
                            .is_some_and(|v| v >= f64::from(self.settings.week_at))
                })
            })
    }
    pub fn reconcile(&mut self, state: &InputState, now: i64) -> Result<Vec<Effect>, ProfileError> {
        Self::validate(state)?;
        let Some(mut old) = self.previous.replace(state.clone()) else {
            return Ok(Vec::new());
        };
        for job in self.active.values().flat_map(|d| &d.jobs) {
            if let Some(own) = state.observations.iter().find(|e| {
                e.observation_id == job.observation_id() && e.generation == job.generation
            }) {
                old.observations.retain(|e| e.profile != job.profile);
                old.observations.push(own.clone());
            }
        }
        if old.same_policy(state) {
            return Ok(Vec::new());
        }
        self.consumed = None;
        if !state.enabled || state.held {
            for decision in self.active.values_mut() {
                decision.rotate = false;
            }
            return Ok(Vec::new());
        }
        // Relevant dependency changes invalidate pending swaps, but their
        // captured jobs remain terminally identifiable when they finish.
        for d in self.active.values_mut() {
            if d.inputs.logins != state.logins || d.inputs.control != state.control {
                d.rotate = false;
            }
        }
        let additions: Vec<_> = state
            .logins
            .iter()
            .filter(|l| l.eligible && !old.logins.contains(l))
            .collect();
        let measurement_changed = state
            .logins
            .iter()
            .any(|l| !same_evidence(state.observation(l), old.observation(l)));
        let control_changed =
            state.enabled != old.enabled || state.held != old.held || state.control != old.control;
        let relief = self.needs_relief(state);
        let rotate =
            measurement_changed || control_changed || (relief && old.logins != state.logins);
        let needed: Vec<_> = state
            .logins
            .iter()
            .filter(|l| {
                l.eligible
                    && (l.live || state.candidate(l))
                    && state.observation(l).is_none()
                    && (additions.contains(l)
                        || (rotate && additions.is_empty() && (relief || l.live))
                        || ((measurement_changed || control_changed) && (relief || l.live)))
            })
            .collect();
        if !rotate && needed.is_empty() {
            return Ok(Vec::new());
        }
        if self.active.len() >= 4096 {
            return Err(ProfileError::RotateDaemon(
                "too many pending usage decisions".into(),
            ));
        }
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| ProfileError::RotateDaemon("decision identifier exhausted".into()))?;
        let id = self.next;
        let jobs: Vec<_> = needed
            .iter()
            .map(|l| {
                self.active
                    .values()
                    .flat_map(|d| &d.jobs)
                    .find(|j| {
                        j.profile == l.profile
                            && j.account_id == l.account_id
                            && j.generation == l.generation
                    })
                    .cloned()
                    .unwrap_or_else(|| ProbeJob {
                        namespace: self.namespace.clone(),
                        decision_id: id,
                        profile: l.profile.clone(),
                        account_id: l.account_id.clone(),
                        generation: l.generation.clone(),
                    })
            })
            .collect();
        let mut effects = vec![Effect::Decision { id }];
        effects.extend(
            jobs.iter()
                .filter(|j| j.decision_id == id)
                .cloned()
                .map(Effect::Probe),
        );
        let decision = Decision {
            inputs: state.clone(),
            attempted: jobs
                .iter()
                .map(|j| (j.profile.clone(), j.generation.clone()))
                .collect(),
            jobs,
            rotate,
            candidates: additions.is_empty() || measurement_changed || control_changed,
            failed: false,
        };
        self.active.insert(id, decision);
        effects.extend(self.finish(id, state, now));
        Ok(effects)
    }
    fn finish(&mut self, id: u64, state: &InputState, _now: i64) -> Vec<Effect> {
        let Some(d) = self.active.get(&id) else {
            return Vec::new();
        };
        if !d.jobs.is_empty() {
            return Vec::new();
        }
        let mut d = self.active.remove(&id).expect("decision exists");
        if !state.enabled
            || state.held
            || !d.rotate
            || d.failed
            || d.inputs.logins != state.logins
            || d.inputs.control != state.control
        {
            return Vec::new();
        }
        // The live probe can reveal the need for candidate evidence. Continue
        // its original decision, reusing in-flight work and never retrying a
        // generation already attempted by this decision.
        let mut effects = Vec::new();
        if d.candidates && self.needs_relief(state) {
            for login in state.logins.iter().filter(|l| state.candidate(l)) {
                if state.observation(login).is_some()
                    || !d
                        .attempted
                        .insert((login.profile.clone(), login.generation.clone()))
                {
                    continue;
                }
                let existing = self
                    .active
                    .values()
                    .flat_map(|d| &d.jobs)
                    .find(|j| {
                        j.profile == login.profile
                            && j.generation == login.generation
                            && j.account_id == login.account_id
                    })
                    .cloned();
                let job = existing.unwrap_or_else(|| ProbeJob {
                    namespace: self.namespace.clone(),
                    decision_id: id,
                    profile: login.profile.clone(),
                    account_id: login.account_id.clone(),
                    generation: login.generation.clone(),
                });
                if job.decision_id == id {
                    effects.push(Effect::Probe(job.clone()));
                }
                d.jobs.push(job);
            }
            if !d.jobs.is_empty() {
                self.active.insert(id, d);
                return effects;
            }
        }
        // Unresolved organisation identity never authorises automatic movement.
        let Some(source) = state
            .logins
            .iter()
            .find(|l| l.live)
            .and_then(|l| l.organisation.as_ref())
        else {
            return Vec::new();
        };
        if !state.logins.iter().any(|l| {
            !l.live
                && l.eligible
                && l.organisation.as_ref() == Some(source)
                && state.observation(l).is_some()
        }) {
            return Vec::new();
        }
        if self
            .consumed
            .as_ref()
            .is_some_and(|old| old.same_policy(state))
        {
            return Vec::new();
        }
        self.consumed = Some(state.clone());
        vec![Effect::Revalidate {
            id,
            inputs: state.clone(),
        }]
    }
    pub fn complete(
        &mut self,
        job: &ProbeJob,
        result: ProbeResult,
        state: &InputState,
        now: i64,
    ) -> Result<Vec<Effect>, ProfileError> {
        Self::validate(state)?;
        let valid = state.enabled
            && !state.held
            && state.logins.iter().any(|l| {
                l.profile == job.profile
                    && l.account_id == job.account_id
                    && l.generation == job.generation
                    && l.eligible
                    && state.observation(l).is_some()
            });
        let mut resumed = Vec::new();
        for (id, decision) in &mut self.active {
            if let Some(i) = decision.jobs.iter().position(|j| j == job) {
                decision.jobs.remove(i);
                if result != ProbeResult::Published || !valid {
                    decision.failed = true;
                }
                resumed.push(*id);
            }
        }
        if resumed.is_empty() {
            return Ok(Vec::new());
        }
        // Consume only this job's own publication. Any other external input
        // remains a change and is reconciled below rather than discarded.
        if let Some(previous) = &mut self.previous
            && result == ProbeResult::Published
            && state.observations.iter().any(|e| {
                e.profile == job.profile
                    && e.generation == job.generation
                    && e.observation_id == job.observation_id()
            })
        {
            previous
                .observations
                .retain(|e| !(e.profile == job.profile && e.generation == job.generation));
            previous.observations.extend(
                state
                    .observations
                    .iter()
                    .filter(|e| e.profile == job.profile && e.generation == job.generation)
                    .cloned(),
            );
        }
        let mut effects = Vec::new();
        for id in resumed {
            effects.extend(self.finish(id, state, now));
        }
        effects.extend(self.reconcile(state, now)?);
        // Several continuations may converge on the same input transaction.
        let mut seen = false;
        effects.retain(|e| {
            if matches!(e, Effect::Revalidate { .. }) {
                if seen {
                    false
                } else {
                    seen = true;
                    true
                }
            } else {
                true
            }
        });
        Ok(effects)
    }
}
