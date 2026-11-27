//! High-use evidence for a failed-probe swap during the post-swap hold.

use super::super::fresh::live::{Point, Series, Window, window};
use super::super::ledger::high_water::HighWater;
use super::super::settings::Settings;
use crate::commands::profile::name::ProfileName;
use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub(in crate::commands::profile::rotate) enum Evidence {
    Level {
        window: &'static str,
        used: f64,
        threshold: u8,
        age: Option<i64>,
        resets_at: Option<i64>,
    },
    Projection {
        first: f64,
        last: f64,
        threshold: u8,
        span: i64,
        age: i64,
        crossing_in: f64,
        horizon: i64,
    },
}

impl fmt::Display for Evidence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Level {
                window,
                used,
                threshold,
                age,
                resets_at,
            } => {
                let source = if age.is_some() {
                    "last reading"
                } else {
                    "peak"
                };
                write!(
                    f,
                    "high use: {window} {source} {used}% (threshold {threshold}%"
                )?;
                if let Some(age) = age {
                    write!(f, ", age {age}s")?;
                }
                if let Some(reset) = resets_at {
                    write!(f, ", reset epoch {reset}")?;
                }
                write!(f, ")")
            }
            Self::Projection {
                first,
                last,
                threshold,
                span,
                age,
                crossing_in,
                horizon,
            } => write!(
                f,
                "high use: session rose {first}% to {last}% over {span}s \
                 (last reading age {age}s, threshold {threshold}% projected \
                 {crossing_in:.1}s after last reading, horizon {horizon}s)"
            ),
        }
    }
}

pub(in crate::commands::profile::rotate) fn known_high(
    series: &Series,
    high_water: &HighWater,
    name: &ProfileName,
    swapped_at: i64,
    now: i64,
    settings: &Settings,
) -> Option<Evidence> {
    if swapped_at > now {
        return None;
    }
    let post_swap = |at| swapped_at < at && at <= now;
    let level = |window, used, threshold, age, resets_at| {
        (f64::is_finite(used) && used >= f64::from(threshold)).then_some(Evidence::Level {
            window,
            used,
            threshold,
            age,
            resets_at,
        })
    };
    let accepted = series.accepted(name).filter(|h| post_swap(h.at));
    if let Some(h) = accepted {
        let age = now.saturating_sub(h.at);
        let session = Point {
            at: h.at,
            session: h.session,
            resets: h.resets,
        };
        if session.open_at(now)
            && let Some(evidence) = level(
                "session",
                h.session,
                settings.session_at,
                Some(age),
                h.resets,
            )
        {
            return Some(evidence);
        }
        // A known weekly reset bounds its reading independently. With no
        // weekly reset time, keep the existing age limit for held readings.
        let week_deadline = h.week_resets.unwrap_or_else(|| session.latest_reset());
        if now < week_deadline
            && let Some(evidence) =
                level("week", h.week, settings.week_at, Some(age), h.week_resets)
        {
            return Some(evidence);
        }
    }
    let peaks = high_water.applicable(
        name.as_str(),
        accepted.and_then(|h| h.resets),
        accepted.and_then(|h| h.week_resets),
        now,
    );
    peaks
        .session
        .and_then(|p| {
            level(
                "session",
                p.used,
                settings.session_at,
                None,
                Some(p.resets_at),
            )
        })
        .or_else(|| {
            let p = peaks.week?;
            level("week", p.used, settings.week_at, None, Some(p.resets_at))
        })
        .or_else(|| {
            let hold = i64::try_from(settings.hold).unwrap_or(i64::MAX);
            let min_span = i64::try_from(settings.interval).unwrap_or(i64::MAX);
            projection(
                series.after_swap(name, swapped_at),
                settings.session_at,
                min_span,
                swapped_at.saturating_add(hold),
                now,
            )
        })
}

/// The trend comes from actual session observations at least one configured
/// interval apart in one still-open window.
/// The projection begins at the last reading and ends at the earliest
/// applicable deadline, even if a reading gave no reset time.
fn projection(
    mut points: impl Iterator<Item = Point>,
    threshold: u8,
    min_span: i64,
    hold_ends: i64,
    now: i64,
) -> Option<Evidence> {
    let last = points.next().filter(|p| p.at <= now && p.open_at(now))?;
    let mut first = last;
    let mut reset = last.resets;
    let mut deadline = hold_ends.min(last.latest_reset());
    for point in std::iter::once(last).chain(points) {
        if !point.open_at(now)
            || matches!(window(reset, point.resets), Window::Earlier | Window::Later)
        {
            break;
        }
        reset = reset.or(point.resets);
        deadline = deadline.min(point.latest_reset());
        if let Some(r) = point.resets {
            deadline = deadline.min(r);
        }
        first = point;
    }
    let span = last.observation_span(&first, min_span)?;
    let rise = last.session - first.session;
    if !rise.is_finite() || rise <= 0.0 || last.session >= f64::from(threshold) {
        return None;
    }
    let horizon = deadline.saturating_sub(last.at);
    let crossing_in = (f64::from(threshold) - last.session) * span as f64 / rise;
    (crossing_in < horizon as f64).then_some(Evidence::Projection {
        first: first.session,
        last: last.session,
        threshold,
        span,
        age: now.saturating_sub(last.at),
        crossing_in,
        horizon,
    })
}
