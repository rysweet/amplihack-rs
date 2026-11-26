//! The swap record (issue #2085): `<rotate dir>/swap.json`, written once per
//! rotator swap so the limit-resume hook can resume sessions stopped by a
//! usage limit. The schema and the hook's reader are in
//! `amplihack_utils::rotator_status::swap_record`. Only a cycle's swap
//! writes one: a `profile switch` and an auto-switch swap never do (D1, D2).

use super::decide::Reading;
use super::ledger::RotateState;
use super::log::Sink;
use crate::commands::profile::ProfileError;
use crate::commands::profile::auto_switch::kit::now;
use crate::commands::profile::name::ProfileName;
use amplihack_utils::rotator_status::swap_record::{SWAP_FILE, SwapReading, SwapRecord};

/// The log line prefix for a swap that wrote no record.
const NOT_WRITTEN: &str = "swap record not written";

/// The log line prefix for a record written with `seq` 1 because the
/// previous one could not be used.
const RESTART: &str = "swap record seq restarts at 1";

/// A percentage as stored: rounded up and clamped to `0..=255`. The caller
/// has checked it is finite.
fn percent(p: f64) -> u8 {
    p.ceil().clamp(0.0, 255.0) as u8
}

/// Writes the record for a swap from `from` to `to` at `t`, whose target
/// reading `reading` was measured from `measured_at`. `Ok(false)`, writing
/// nothing, when the reading is not `Reading::Measured` or a figure is not
/// finite (S8: a NaN must never read as 0% used). `seq` is the previous
/// record's plus one, or 1 when there is none or it cannot be read; a
/// previous record that exists but cannot be read or parsed is one line on
/// `out`. The write is private (0600) and atomic.
#[cfg(test)]
pub(super) fn record_swap(
    rs: &RotateState,
    out: &mut Sink,
    from: &ProfileName,
    to: &ProfileName,
    reading: &Reading,
    measured_at: i64,
    t: i64,
) -> Result<bool, ProfileError> {
    record_with_affinity(
        rs,
        out,
        from,
        to,
        reading,
        measured_at,
        t,
        None,
        None,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
fn record_with_affinity(
    rs: &RotateState,
    out: &mut Sink,
    from: &ProfileName,
    to: &ProfileName,
    reading: &Reading,
    measured_at: i64,
    t: i64,
    from_org: Option<&str>,
    to_org: Option<&str>,
    explicit: bool,
) -> Result<bool, ProfileError> {
    let Reading::Measured { session, week } = *reading else {
        return Ok(false);
    };
    let from_org = from_org.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    let to_org = to_org.filter(|id| amplihack_utils::organisation_affinity::valid_id(id));
    if !session.is_finite() || !week.is_finite() {
        return Ok(false);
    }
    let previous = match rs.read(&rs.dir.join(SWAP_FILE), "swap record") {
        Ok(None) => None,
        Ok(Some(raw)) => match serde_json::from_slice::<SwapRecord>(&raw) {
            Ok(p) => Some(p),
            Err(_) => {
                out.emit(now(), &format!("{RESTART}: previous unparsable"));
                None
            }
        },
        Err(e) => {
            out.emit(now(), &format!("{RESTART}: previous unreadable ({e})"));
            None
        }
    };
    let record = SwapRecord {
        from_org: from_org.map(str::to_owned),
        to_org: to_org.map(str::to_owned),
        affinity: explicit
            .then(|| amplihack_utils::organisation_affinity::Affinity::between(from_org, to_org)),
        seq: previous.map_or(1, |p| p.seq.saturating_add(1)),
        live_login: to.as_str().to_string(),
        from_login: from.as_str().to_string(),
        swapped_at: t,
        reading: SwapReading {
            session: percent(session),
            week: percent(week),
            measured_at,
        },
    };
    let bytes = serde_json::to_vec(&record)
        .map_err(|e| ProfileError::Io(std::io::Error::other(e.to_string())))?;
    rs.write(SWAP_FILE, &bytes)?;
    Ok(true)
}

/// After a cycle's swap is saved, under the rotate lock: writes the record
/// when the ledger on disk shows `to` live. Any reason it is not written is
/// one log line; the swap stands and the cycle outcome is unchanged.
/// `swapped_at` is when `swap_with` returned, never the cycle's start time.
#[allow(clippy::too_many_arguments)]
pub(super) fn after_swap(
    rs: &RotateState,
    out: &mut Sink,
    from: &ProfileName,
    to: &ProfileName,
    others: &[(ProfileName, Reading)],
    measured_at: i64,
    swapped_at: i64,
    organisations: (Option<&str>, Option<&str>),
) {
    let live = match rs.load_ledger() {
        Ok(Some(l)) => l.live,
        Ok(None) => None,
        Err(e) => {
            out.emit(now(), &format!("{NOT_WRITTEN}: ledger: {e}"));
            return;
        }
    };
    if live.as_deref() != Some(to.as_str()) {
        out.emit(
            now(),
            &format!("{NOT_WRITTEN}: the ledger does not show {to} live"),
        );
        return;
    }
    let Some((_, reading)) = others.iter().find(|(n, _)| n == to) else {
        out.emit(now(), &format!("{NOT_WRITTEN}: no reading of {to}"));
        return;
    };
    match record_with_affinity(
        rs,
        out,
        from,
        to,
        reading,
        measured_at,
        swapped_at,
        organisations.0,
        organisations.1,
        true,
    ) {
        Ok(true) => {}
        Ok(false) => out.emit(
            now(),
            &format!("{NOT_WRITTEN}: {to} has no finite measured reading"),
        ),
        Err(e) => out.emit(now(), &format!("{NOT_WRITTEN}: {e}")),
    }
}
