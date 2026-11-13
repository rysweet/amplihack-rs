//! #2029: --interval remains validated input but cannot schedule work.
use super::TuneFlags;
use super::settings::resolve;

#[test]
fn accepted_legacy_intervals_are_not_forwarded_as_a_daemon_schedule() {
    for interval in ["1", "60", "86400"] {
        let settings = resolve(&TuneFlags::default(), Some(interval), &|_| None).unwrap();
        for with_interval in [false, true] {
            let args = settings.to_args(with_interval);
            assert!(
                !args
                    .iter()
                    .any(|s| s == "--interval" || s.starts_with("--interval=")),
                "deprecated interval escaped into daemon arguments: {args:?}"
            );
        }
    }
}

#[test]
fn deprecated_interval_still_rejects_invalid_flag_and_environment_values() {
    for invalid in ["0", "86401", "-1", "1.5", " 60", "tomorrow"] {
        assert!(resolve(&TuneFlags::default(), Some(invalid), &|_| None).is_err());
        let environment =
            |name: &str| (name == "AMPLIHACK_ROTATE_INTERVAL").then(|| invalid.into());
        assert!(resolve(&TuneFlags::default(), None, &environment).is_err());
    }
}
