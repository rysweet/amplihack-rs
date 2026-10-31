//! Read explicit files and discard record contents before aggregation.
use super::{
    adapters::{self, Event},
    aggregate,
    manifest::validate,
    models::*,
};
use anyhow::Result;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

pub(super) struct Observation {
    pub session: usize,
    pub timestamp: Option<String>,
    pub event: Event,
    pub incomplete: bool,
}
pub(super) fn diagnostic(
    ds: &mut BTreeSet<Diagnostic>,
    session: Option<usize>,
    file: Option<usize>,
    line: Option<usize>,
    code: &str,
) {
    ds.insert(Diagnostic {
        session,
        file,
        line,
        code: code.to_owned(),
    });
}

pub fn collect(m: &Manifest, base: &Path) -> Result<Report> {
    validate(m)?;
    let mut ds = BTreeSet::new();
    let mut observations = Vec::new();
    for (si, s) in m.sessions.iter().enumerate() {
        if !s.complete {
            diagnostic(
                &mut ds,
                Some(si),
                None,
                None,
                "session_coverage_unconfirmed",
            );
        }
        if s.run_id.is_none() {
            diagnostic(&mut ds, Some(si), None, None, "unattributed_run");
        }
        if s.parent
            .as_ref()
            .is_some_and(|p| !m.sessions.iter().any(|s| &s.id == p))
        {
            diagnostic(&mut ds, Some(si), None, None, "missing_parent");
        }
        if s.files.is_empty() {
            diagnostic(&mut ds, Some(si), None, None, "missing_files");
        }
        for (fi, path) in s.files.iter().enumerate() {
            if !std::fs::metadata(base.join(path)).is_ok_and(|meta| meta.is_file()) {
                diagnostic(&mut ds, Some(si), Some(fi), None, "unreadable_file");
                continue;
            }
            let Ok(file) = File::open(base.join(path)) else {
                diagnostic(&mut ds, Some(si), Some(fi), None, "unreadable_file");
                continue;
            };
            let mut native = None;
            let mut model = None;
            for (li, line) in BufReader::new(file).lines().enumerate() {
                let Ok(line) = line else {
                    diagnostic(
                        &mut ds,
                        Some(si),
                        Some(fi),
                        Some(li + 1),
                        "unreadable_record",
                    );
                    break;
                };
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str(&line) else {
                    diagnostic(
                        &mut ds,
                        Some(si),
                        Some(fi),
                        Some(li + 1),
                        "invalid_json_record",
                    );
                    continue;
                };
                let parsed = adapters::parse(s.provider, &v);
                let incomplete = !parsed.diagnostics.is_empty();
                for code in parsed.diagnostics {
                    diagnostic(&mut ds, Some(si), Some(fi), Some(li + 1), code);
                }
                for mut event in parsed.event.into_iter().chain(parsed.snapshot) {
                    match &mut event {
                        Event::NativeSession(id) => {
                            if native.as_ref() != Some(id) {
                                model = None;
                            }
                            native = Some(id.clone());
                            continue;
                        }
                        Event::Model(value) => {
                            model = Some(value.clone());
                            continue;
                        }
                        Event::Usage {
                            native_session,
                            model: event_model,
                            ..
                        } => {
                            if native_session.is_none() {
                                *native_session = native.clone();
                            }
                            // A turn context belongs to this file's native
                            // session, never to a copied foreign response.
                            if event_model.is_none()
                                && native.is_some()
                                && *native_session == native
                            {
                                *event_model = model.clone();
                            }
                            if native_session.is_none() {
                                diagnostic(
                                    &mut ds,
                                    Some(si),
                                    Some(fi),
                                    Some(li + 1),
                                    "missing_native_session_identity",
                                );
                            }
                        }
                    }
                    observations.push(Observation {
                        session: si,
                        timestamp: parsed.timestamp.clone(),
                        event,
                        incomplete,
                    });
                }
            }
        }
    }
    observations.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
    Ok(aggregate::aggregate(m, observations, ds))
}
