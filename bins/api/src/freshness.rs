//! How old the persisted market snapshot may be before paper opens are
//! refused (DEC-0016).
//!
//! The engine writes a snapshot with every scan. If it stops (crash, stalled
//! feed, deploy) the API must not keep filling paper orders against old books,
//! so `POST /positions` requires a snapshot newer than this age. The default is
//! three scan intervals.

use chrono::{DateTime, Duration, Utc};

/// Default maximum snapshot age, in seconds.
pub const DEFAULT_MAX_SNAPSHOT_AGE_SECS: i64 = 90;

/// Parse `PERPETO_MAX_SNAPSHOT_AGE_SECS`: unset means the default; anything
/// else must be a positive whole number of seconds.
///
/// # Errors
/// Returns a message when the value is not a positive integer.
pub fn parse_max_snapshot_age(value: Option<&str>) -> Result<Duration, String> {
    let Some(raw) = value else {
        return Ok(Duration::seconds(DEFAULT_MAX_SNAPSHOT_AGE_SECS));
    };
    match raw.trim().parse::<i64>() {
        Ok(seconds) if seconds > 0 => Ok(Duration::seconds(seconds)),
        _ => Err(format!(
            "PERPETO_MAX_SNAPSHOT_AGE_SECS must be a positive integer, got {raw:?}"
        )),
    }
}

/// Whether a snapshot written at `recorded_at` may still back an open at `now`.
/// A snapshot from the future (clock skew) is treated as fresh only within the
/// same bound.
#[must_use]
pub fn is_fresh(recorded_at: DateTime<Utc>, now: DateTime<Utc>, max_age: Duration) -> bool {
    let age = now - recorded_at;
    age <= max_age && age >= -max_age
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 22, 12, 0, second)
            .single()
            .unwrap_or_default()
    }

    #[test]
    fn defaults_to_ninety_seconds() {
        assert_eq!(parse_max_snapshot_age(None), Ok(Duration::seconds(90)));
        assert_eq!(
            parse_max_snapshot_age(Some(" 30 ")),
            Ok(Duration::seconds(30))
        );
    }

    #[test]
    fn rejects_non_positive_or_malformed_values() {
        for value in ["0", "-5", "abc", "", "1.5"] {
            assert!(parse_max_snapshot_age(Some(value)).is_err(), "{value}");
        }
    }

    #[test]
    fn a_snapshot_is_fresh_up_to_and_including_the_limit() {
        let limit = Duration::seconds(30);
        assert!(is_fresh(at(0), at(30), limit));
        assert!(!is_fresh(at(0), at(31), limit));
        assert!(is_fresh(at(10), at(0), limit));
        assert!(!is_fresh(at(59), at(0), limit));
    }
}
