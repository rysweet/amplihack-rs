//! Test-only read access to the private state of a live `Series`.

use super::Series;

impl Series {
    pub fn len(&self) -> usize {
        self.points.len()
    }

    /// Cycles in a row the cached figures have been unchanged.
    pub fn unchanged(&self) -> u32 {
        self.unchanged
    }

    /// Whether a disagreeing re-read has marked the cache as behind.
    pub fn behind(&self) -> bool {
        self.behind
    }
}
