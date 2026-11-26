//! Weekly decision figures, independent of the all-models reset window.

use super::Windows;

/// The largest weekly percentage and its origin. `None` means all models.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DecidingWeek<'a> {
    pub used: f64,
    pub model: Option<&'a str>,
}

impl Windows {
    /// Equal maxima prefer all models, then model labels in byte order.
    pub fn deciding_week(&self) -> Option<DecidingWeek<'_>> {
        let mut deciding = DecidingWeek {
            used: self.week?.used,
            model: None,
        };
        for w in &self.week_models {
            if w.used_percentage > deciding.used
                || (w.used_percentage == deciding.used
                    && deciding.model.is_some_and(|m| w.label.as_str() < m))
            {
                deciding = DecidingWeek {
                    used: w.used_percentage,
                    model: Some(&w.label),
                };
            }
        }
        Some(deciding)
    }
}
