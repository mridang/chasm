use crate::random::Random;
use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc};

/// Approximate milliseconds in a calendar year, ignoring leap years.
///
/// Used to derive a counterpart bound when only one of `min`/`max` is supplied
/// so the generator still samples from a finite window.
const MS_PER_YEAR: i64 = 365 * 24 * 60 * 60 * 1000;

/// Generates an ISO 8601 date-time string with UTC offset.
///
/// When neither bound is supplied returns the legacy constant value used by snapshot
/// fixtures; otherwise samples uniformly in the inclusive range `[min, max]`.
pub fn generate_datetime(rng: &mut Random) -> String {
    generate_datetime_with_bounds(rng, None, None)
}

/// Generates an ISO 8601 date-time string honouring optional min/max bounds.
///
/// Both bounds are parsed leniently: RFC3339 timestamps first, then date-only `YYYY-MM-DD`
/// values treated as midnight UTC. When parsing fails or no bounds are supplied the
/// generator falls back to the legacy constant string so downstream snapshot tests are
/// stable.
pub fn generate_datetime_with_bounds(
    rng: &mut Random,
    min: Option<&str>,
    max: Option<&str>,
) -> String {
    let min_dt = min.and_then(parse_flexible);
    let max_dt = max.and_then(parse_flexible);
    match (min_dt, max_dt) {
        (Some(lo), Some(hi)) if hi >= lo => {
            let lo_ms = lo.timestamp_millis();
            let hi_ms = hi.timestamp_millis();
            let pick = rng.int(lo_ms, hi_ms);
            format_datetime(pick)
        }
        (Some(lo), None) => {
            let lo_ms = lo.timestamp_millis();
            let hi_ms = lo_ms.saturating_add(MS_PER_YEAR);
            let pick = rng.int(lo_ms, hi_ms);
            format_datetime(pick)
        }
        (None, Some(hi)) => {
            let hi_ms = hi.timestamp_millis();
            let lo_ms = hi_ms.saturating_sub(MS_PER_YEAR);
            let pick = rng.int(lo_ms, hi_ms);
            format_datetime(pick)
        }
        _ => "2024-01-15T12:00:00Z".to_string(),
    }
}

/// Parses an RFC3339 timestamp or a `YYYY-MM-DD` date string into a UTC `DateTime`.
fn parse_flexible(s: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let midnight = NaiveTime::from_hms_opt(0, 0, 0)?;
        let ndt = NaiveDateTime::new(d, midnight);
        return Utc.from_local_datetime(&ndt).single();
    }
    None
}

/// Formats a UTC millisecond timestamp as an ISO 8601 string with `Z` suffix.
fn format_datetime(ms: i64) -> String {
    let dt = Utc
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Utc::now);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        dt.year(),
        dt.month(),
        dt.day(),
        dt.hour(),
        dt.minute(),
        dt.second()
    )
}

/// Generates an ISO 8601 date string in `YYYY-MM-DD` format.
pub fn generate_date(rng: &mut Random) -> String {
    let year = rng.int(2000, 2030);
    let month = rng.int(1, 12);
    let day = rng.int(1, 28);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Generates an ISO 8601 time string in `HH:MM:SS` format.
pub fn generate_time(rng: &mut Random) -> String {
    let hour = rng.int(0, 23);
    let minute = rng.int(0, 59);
    let second = rng.int(0, 59);
    format!("{:02}:{:02}:{:02}", hour, minute, second)
}
