//! Durable conflict delivery, keyed by verified accounts and file evidence.
use super::*;

pub(in crate::commands::profile::rotate) fn clear_conflict(
    rs: &RotateState,
    name: &ProfileName,
) -> Result<(), ProfileError> {
    let mut s = load(rs)?;
    if s.conflicts.remove(name.as_str()).is_some() {
        save(rs, &s)?;
    }
    Ok(())
}
pub(in crate::commands::profile::rotate) fn conflict(
    rs: &RotateState,
    name: &ProfileName,
    a: &Account,
    b: &Account,
    live_file: &crate::commands::profile::rotate::login_identity::Stamp,
    occupied_file: &crate::commands::profile::rotate::login_identity::Stamp,
) -> Result<(), ProfileError> {
    let mut s = load(rs)?;
    if !s.conflicts.get(name.as_str()).is_some_and(|c| {
        c.live.names(a)
            && c.occupied.names(b)
            && &c.live_file == live_file
            && &c.occupied_file == occupied_file
    }) {
        s.conflicts.insert(
            name.to_string(),
            Conflict {
                live: Identity::new(a),
                occupied: Identity::new(b),
                live_file: live_file.clone(),
                occupied_file: occupied_file.clone(),
                alert_id: uuid::Uuid::new_v4().to_string(),
                alerted: false,
            },
        );
        save(rs, &s)?;
    }
    let c = s.conflicts.get(name.as_str()).ok_or_else(bad)?;
    if !c.alerted {
        let label = |a: &Account| {
            let shown = a
                .block()
                .get("emailAddress")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(a.key())
                .chars()
                .filter(|c| !c.is_control())
                .take(320)
                .collect::<String>();
            format!(
                "{shown} (account={}, organisation={})",
                a.key(),
                a.org().unwrap_or("unknown")
            )
        };
        deliver(
            rs,
            &c.alert_id,
            &format!(
                "ALERT: duplicate conflict for {name}: live account {}, occupied account {}; both logins preserved",
                label(a),
                label(b)
            ),
        )?;
        s.conflicts.get_mut(name.as_str()).ok_or_else(bad)?.alerted = true;
        save(rs, &s)?;
    }
    Ok(())
}
