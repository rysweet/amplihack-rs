//! Durable, replayable alert delivery in the existing rotation log.
use super::*;

impl Sink {
    /// Durable keyed delivery under the state lock. Replaying an interrupted
    /// acknowledgement recognises the key in the current or rotated log.
    pub(in crate::commands::profile::rotate) fn keyed(
        &mut self,
        t: i64,
        key: &str,
        message: &str,
    ) -> io::Result<()> {
        let log = self
            .log
            .as_mut()
            .ok_or_else(|| refused("unavailable for keyed alert"))?;
        let marker = format!("[login-alert:{key}]");
        for path in [&log.path, &log.rotated] {
            if let Some((raw, m)) =
                crate::commands::profile::auto_switch::kit::read_bounded_nofollow(
                    path,
                    MAX_LOG + 65536,
                )?
            {
                if m.uid() != euid() || m.mode() & 0o077 != 0 {
                    return Err(refused("unsafe keyed alert history"));
                }
                if String::from_utf8_lossy(&raw)
                    .lines()
                    .any(|l| l.ends_with(&marker))
                {
                    log.file.sync_all()?;
                    return Ok(());
                }
            }
        }
        log.write(t, &format!("{message} {marker}"))?;
        log.file.sync_all()?;
        crate::commands::profile::auto_switch::kit::fsync_dir(log.path.parent().unwrap())?;
        self.print(t, message);
        Ok(())
    }
}
