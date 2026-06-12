//! Percent / date / relative-time formatting (`chrono`-based, locale-aware where
//! the platform exposes a locale). Mirrors PitStop's `Format` helpers.

use chrono::{DateTime, Datelike, Duration, Local};

/// `0.42` → `"42%"`, `None` → `"–"`.
pub fn percent(utilization: Option<f64>) -> String {
    match utilization {
        Some(u) => format!("{}%", (u * 100.0).round() as i64),
        None => "–".to_string(),
    }
}

/// Whether two instants fall on the same local calendar day.
fn same_local_day(a: DateTime<Local>, b: DateTime<Local>) -> bool {
    a.year() == b.year() && a.ordinal() == b.ordinal()
}

/// Wall-clock time, day-qualified when the target is not today.
/// `"9:49 PM"` today, `"Thu 10:29 AM"` otherwise.
fn clock(target: DateTime<Local>, now: DateTime<Local>) -> String {
    if same_local_day(target, now) {
        target.format("%-l:%M %p").to_string()
    } else {
        target.format("%a %-l:%M %p").to_string()
    }
}

/// `"resets 9:49 PM (in 3h 34m)"`, day-qualified when not today.
pub fn reset(target: DateTime<Local>, now: DateTime<Local>) -> String {
    format!(
        "resets {} (in {})",
        clock(target, now),
        relative_short(target, now)
    )
}

/// Long relative form: `"in 3h 34m"` / `"in 5d 16h"`; past → `"now"`.
pub fn relative(target: DateTime<Local>, now: DateTime<Local>) -> String {
    let r = relative_short(target, now);
    if r == "now" {
        r
    } else {
        format!("in {r}")
    }
}

/// Short relative form: `"3h 34m"`, `"5d 16h"`, `"45m"`, `"now"`.
pub fn relative_short(target: DateTime<Local>, now: DateTime<Local>) -> String {
    let delta = target.signed_duration_since(now);
    if delta <= Duration::zero() {
        return "now".to_string();
    }
    let days = delta.num_days();
    let hours = delta.num_hours() - days * 24;
    let mins = delta.num_minutes() - delta.num_hours() * 60;

    if days > 0 {
        format!("{days}d {hours}h")
    } else if delta.num_hours() > 0 {
        format!("{}h {}m", delta.num_hours(), mins)
    } else {
        format!("{}m", delta.num_minutes().max(1))
    }
}

/// Compact reset for usage bars: `"9:49 PM · 3h 34m"` /
/// `"Thu 10:29 AM · 5d 16h"`.
pub fn compact_reset(target: DateTime<Local>, now: DateTime<Local>) -> String {
    format!("{} · {}", clock(target, now), relative_short(target, now))
}

/// `HH:MM:SS` for the "Updated …" line and stale stamps.
pub fn updated(target: DateTime<Local>) -> String {
    target.format("%H:%M:%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn percent_rounds_and_handles_none() {
        assert_eq!(percent(Some(0.425)), "43%");
        assert_eq!(percent(Some(0.0)), "0%");
        assert_eq!(percent(None), "–");
    }

    #[test]
    fn relative_short_buckets() {
        let now = local(2026, 6, 13, 12, 0);
        assert_eq!(relative_short(now + Duration::minutes(45), now), "45m");
        assert_eq!(
            relative_short(now + Duration::hours(3) + Duration::minutes(34), now),
            "3h 34m"
        );
        assert_eq!(
            relative_short(now + Duration::days(5) + Duration::hours(16), now),
            "5d 16h"
        );
        assert_eq!(relative_short(now - Duration::minutes(5), now), "now");
    }

    #[test]
    fn compact_reset_includes_clock_and_delta() {
        let now = local(2026, 6, 13, 12, 0);
        let target = local(2026, 6, 13, 15, 34);
        assert_eq!(compact_reset(target, now), "3:34 PM · 3h 34m");
    }
}
