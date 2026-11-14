//! Readers of what the stub `claude` writes to `$RECORD_DIR`: its start and
//! end marks and its hold files. See `issue_1811_fixture` for the stub.

use std::collections::HashSet;
use std::fs;

impl super::Fixture {
    /// How many times the stub `claude` started.
    pub fn claude_runs(&self) -> usize {
        fs::read_to_string(self.records.join("marks"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.starts_with("S "))
            .count()
    }

    /// Pids of stubs that are running, admitted and held (`$STUB_HOLD`),
    /// leaving out those in `released`.
    pub fn held(&self, released: &HashSet<String>) -> Vec<String> {
        let mut pids: Vec<String> = fs::read_dir(&self.records)
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|e| {
                        e.file_name()
                            .to_str()
                            .and_then(|n| n.strip_prefix("ready."))
                            .map(str::to_string)
                    })
                    .filter(|pid| !released.contains(pid))
                    .collect()
            })
            .unwrap_or_default();
        pids.sort();
        pids
    }

    /// Lets the held stub `pid` finish: opening its FIFO for writing and
    /// closing it ends the stub's read.
    pub fn release(&self, pid: &str) {
        use crate::support::unix_process::{process_alive, release_fifo};
        let consumer = pid.parse().expect("stub pid");
        release_fifo(&self.records.join(format!("hold.{pid}")), || {
            process_alive(consumer)
        });
    }

    /// The largest number of stub `claude` processes running at once.
    pub fn max_overlap(&self) -> usize {
        let marks = fs::read_to_string(self.records.join("marks")).unwrap_or_default();
        let mut events: Vec<(u128, i32)> = marks
            .lines()
            .filter_map(|l| {
                let (k, t) = l.split_once(' ')?;
                let t: u128 = t.trim().parse().ok()?;
                Some((t, if k == "S" { 1 } else { -1 }))
            })
            .collect();
        // At equal times, count an end before a start.
        events.sort();
        let (mut cur, mut max) = (0i32, 0i32);
        for (_, d) in events {
            cur += d;
            max = max.max(cur);
        }
        max as usize
    }
}
