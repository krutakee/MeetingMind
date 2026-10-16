//! UTC timestamps as RFC 3339 text (`2026-10-16T09:05:03.250Z`) without a date library.
//!
//! Uses the civil-from-days algorithm (Howard Hinnant, public domain), valid for the whole range we care about.

use std::time::{SystemTime, UNIX_EPOCH};

/// Formats a point in time as `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC). Times before 1970 are shown as the epoch.
pub fn rfc3339_utc(time: SystemTime) -> String {
    let since_epoch = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let total_secs = since_epoch.as_secs();
    let millis = since_epoch.subsec_millis();

    let days = (total_secs / 86_400) as i64;
    let secs_of_day = total_secs % 86_400;
    let (year, month, day) = civil_from_days(days);

    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Days since 1970-01-01 to (year, month, day) in the proleptic Gregorian calendar.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, 0..=146096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year, March-based
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn at(secs: u64, millis: u32) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs) + Duration::from_millis(millis as u64)
    }

    #[test]
    fn the_epoch_is_1970() {
        assert_eq!(rfc3339_utc(at(0, 0)), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn known_dates_are_formatted_correctly() {
        // Checked against independent references.
        assert_eq!(
            rfc3339_utc(at(951_782_400, 0)),
            "2000-02-29T00:00:00.000Z",
            "a leap day in a 400-year leap year"
        );
        assert_eq!(
            rfc3339_utc(at(1_000_000_000, 0)),
            "2001-09-09T01:46:40.000Z"
        );
        assert_eq!(
            rfc3339_utc(at(1_709_164_800, 0)),
            "2024-02-29T00:00:00.000Z"
        );
        assert_eq!(
            rfc3339_utc(at(1_735_689_599, 999)),
            "2024-12-31T23:59:59.999Z"
        );
        assert_eq!(
            rfc3339_utc(at(1_735_689_600, 0)),
            "2025-01-01T00:00:00.000Z"
        );
        assert_eq!(
            rfc3339_utc(at(4_102_444_800, 0)),
            "2100-01-01T00:00:00.000Z",
            "2100 is not a leap year"
        );
    }

    #[test]
    fn milliseconds_are_zero_padded() {
        assert_eq!(
            rfc3339_utc(at(1_000_000_000, 5)),
            "2001-09-09T01:46:40.005Z"
        );
    }

    #[test]
    fn a_time_before_the_epoch_does_not_panic() {
        assert_eq!(
            rfc3339_utc(UNIX_EPOCH - Duration::from_secs(10)),
            "1970-01-01T00:00:00.000Z"
        );
    }

    #[test]
    fn every_day_over_a_century_round_trips_in_order() {
        // Consecutive days must produce strictly increasing dates, and month lengths must be right.
        let mut previous = String::new();
        for day in 0..36_525i64 {
            let text = rfc3339_utc(at((day as u64) * 86_400, 0));
            assert!(text > previous, "{text} not after {previous}");
            previous = text;
        }
        assert_eq!(&previous[..10], "2069-12-31");
    }
}
