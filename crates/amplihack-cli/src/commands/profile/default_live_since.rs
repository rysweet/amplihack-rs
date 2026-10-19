//! A rotator ledger's `since` as epoch seconds (issue #1592, D14): a number
//! (the `profile rotate` ledger), or an ISO-8601 time with an offset or in
//! local time (the prototype writes `datetime.now().isoformat()`).

use chrono::TimeZone;

pub(super) fn epoch_secs(v: &serde_json::Value) -> Option<f64> {
    let serde_json::Value::String(s) = v else {
        return v.as_f64();
    };
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp() as f64);
    }
    let naive = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f").ok()?;
    let t = chrono::Local.from_local_datetime(&naive).earliest()?;
    Some(t.timestamp() as f64)
}
