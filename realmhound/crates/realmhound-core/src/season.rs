//! Pure time helpers for the season / battlepass timers and the
//! end-of-cycle pinned warnings.
//!
//! Everything here is `now`-injected and side-effect free so it can be unit
//! tested (leap years, month rollover, boundary conditions) without a clock.

use chrono::{DateTime, Datelike, Duration, TimeZone, Timelike, Utc};

use crate::settings::UtcResetTime;

/// Convert a user-entered [`UtcResetTime`] into an absolute UTC instant.
/// Returns `None` when unset or the fields don't form a valid date/time.
pub fn reset_to_datetime(reset: &UtcResetTime) -> Option<DateTime<Utc>> {
    if !reset.is_set() {
        return None;
    }
    match Utc.with_ymd_and_hms(
        reset.year,
        reset.month,
        reset.day,
        reset.hour,
        reset.minute,
        0,
    ) {
        chrono::LocalResult::Single(dt) => Some(dt),
        _ => None,
    }
}

/// Convert a unix timestamp (seconds) into a [`UtcResetTime`]. Returns `None`
/// for non-positive or out-of-range timestamps. Rounded to the nearest minute
/// (the reset editor works at minute granularity), so an `endDate` like
/// `08:59:59` becomes `09:00` rather than expiring a minute early.
pub fn unix_to_reset(secs: i64) -> Option<UtcResetTime> {
    if secs <= 0 {
        return None;
    }
    let dt = match Utc.timestamp_opt(secs + 30, 0) {
        chrono::LocalResult::Single(dt) => dt,
        _ => return None,
    };
    Some(UtcResetTime {
        year: dt.year(),
        month: dt.month(),
        day: dt.day(),
        hour: dt.hour(),
        minute: dt.minute(),
    })
}

/// Snap an instant to the nearest Tuesday, preserving the time of day. RotMG
/// season and battlepass boundaries always land on a Tuesday, so estimated
/// boundaries are snapped to remove the up-to-a-few-days error of a raw midpoint.
pub fn snap_to_tuesday(dt: DateTime<Utc>) -> DateTime<Utc> {
    // chrono weekday: Tuesday.num_days_from_monday() == 1.
    let offset = dt.weekday().num_days_from_monday() as i64 - 1; // days since Tuesday
                                                                 // Map to nearest: offset in [-6..6]; choose the smaller absolute shift.
    let shift = match offset {
        0 => 0,
        1..=3 => -offset,    // 1..3 days after Tuesday -> go back
        4..=6 => 7 - offset, // 4..6 days after Tuesday -> go forward to next Tuesday
        _ => 0,
    };
    dt + Duration::days(shift)
}

/// Resolve the effective battlepass end instant.
///
/// A hand-entered `battlepass_reset` overrides everything. Otherwise the end is
/// estimated: each season holds two equal battlepasses, so the first ends at the
/// season midpoint (snapped to the nearest Tuesday) and the second ends with the
/// season. Falls back to the shared season `reset` when the span is unknown.
pub fn battlepass_target(
    cfg: &crate::settings::SeasonConfig,
    now: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    if cfg.battlepass_reset.is_set() {
        return reset_to_datetime(&cfg.battlepass_reset);
    }
    if cfg.auto_season_start_unix > 0 && cfg.auto_season_end_unix > cfg.auto_season_start_unix {
        let mid = (cfg.auto_season_start_unix + cfg.auto_season_end_unix) / 2;
        if let chrono::LocalResult::Single(mid_dt) = Utc.timestamp_opt(mid, 0) {
            let bp1_end = snap_to_tuesday(mid_dt);
            if now < bp1_end {
                return Some(bp1_end);
            }
            if let chrono::LocalResult::Single(end_dt) =
                Utc.timestamp_opt(cfg.auto_season_end_unix, 0)
            {
                return Some(end_dt);
            }
        }
    }
    reset_to_datetime(&cfg.reset)
}

/// Time remaining until `target`, or `None` if `target` is at/behind `now`.
pub fn remaining(now: DateTime<Utc>, target: DateTime<Utc>) -> Option<Duration> {
    let d = target - now;
    if d > Duration::zero() {
        Some(d)
    } else {
        None
    }
}

/// The daily-login calendar end: 23:59 UTC on the last day of the current month
/// (one minute before the next month begins). The final calendar rewards remain
/// claimable through the end of the last day, so the countdown targets this
/// instant rather than the following midnight.
pub fn daily_calendar_end(now: DateTime<Utc>) -> DateTime<Utc> {
    let (year, month) = if now.month() == 12 {
        (now.year() + 1, 1)
    } else {
        (now.year(), now.month() + 1)
    };
    // First day of next month at 00:00 always exists; back up one minute to land
    // on 23:59 UTC of the last day of the current month.
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0).unwrap() - Duration::minutes(1)
}

/// Whether `now` is within the final `window_hours` before `target` (and not
/// yet past it). Used to decide when to pin an end-of-cycle warning.
pub fn within_final_window(now: DateTime<Utc>, target: DateTime<Utc>, window_hours: i64) -> bool {
    match remaining(now, target) {
        Some(d) => d <= Duration::hours(window_hours),
        None => false,
    }
}

/// Format a remaining duration compactly, e.g. "6d 19h", "19h 5m", "5m", "<1m".
pub fn format_remaining(d: Duration) -> String {
    let total_minutes = d.num_minutes().max(0);
    let days = total_minutes / (24 * 60);
    let hours = (total_minutes % (24 * 60)) / 60;
    let minutes = total_minutes % 60;

    if days > 0 {
        format!("{}d {}h", days, hours)
    } else if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else if minutes > 0 {
        format!("{}m", minutes)
    } else {
        "<1m".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
    }

    #[test]
    fn reset_conversion_and_unset() {
        let unset = UtcResetTime::default();
        assert!(reset_to_datetime(&unset).is_none());

        let set = UtcResetTime {
            year: 2025,
            month: 6,
            day: 15,
            hour: 0,
            minute: 0,
        };
        assert_eq!(reset_to_datetime(&set), Some(dt(2025, 6, 15, 0, 0)));

        // Invalid date rejected.
        let bad = UtcResetTime {
            year: 2025,
            month: 13,
            day: 40,
            hour: 0,
            minute: 0,
        };
        assert!(reset_to_datetime(&bad).is_none());
    }

    #[test]
    fn unix_to_reset_matches_known_season_end() {
        // Season 52 (The Mad God's Tempest) endDate = 2026-10-06 08:59:59 UTC,
        // rounded to the nearest minute -> 09:00.
        let r = unix_to_reset(1791277199).unwrap();
        assert_eq!(
            r,
            UtcResetTime {
                year: 2026,
                month: 10,
                day: 6,
                hour: 9,
                minute: 0
            }
        );
        // Round-trips back through reset_to_datetime at minute granularity.
        assert_eq!(reset_to_datetime(&r), Some(dt(2026, 10, 6, 9, 0)));
    }

    #[test]
    fn unix_to_reset_rejects_non_positive() {
        assert!(unix_to_reset(0).is_none());
        assert!(unix_to_reset(-1).is_none());
    }

    #[test]
    fn snap_to_tuesday_picks_nearest() {
        use chrono::Datelike;
        // 2026-09-04 is a Friday; nearest Tuesday is 2026-09-01, time preserved.
        let fri = dt(2026, 9, 4, 10, 58);
        let snapped = snap_to_tuesday(fri);
        assert_eq!(snapped, dt(2026, 9, 1, 10, 58));
        assert_eq!(snapped.weekday(), chrono::Weekday::Tue);
        // A Tuesday stays put; a Saturday rounds forward to the next Tuesday.
        assert_eq!(snap_to_tuesday(dt(2026, 9, 1, 9, 0)), dt(2026, 9, 1, 9, 0));
        assert_eq!(snap_to_tuesday(dt(2026, 9, 5, 9, 0)), dt(2026, 9, 8, 9, 0));
    }

    #[test]
    fn battlepass_estimate_matches_real_season_52() {
        use crate::settings::SeasonConfig;
        // Season 52: start 2026-08-03, end 2026-10-06 -> BP1 ends Tue 2026-09-01.
        let cfg = SeasonConfig {
            auto_season_start_unix: 1785761800,
            auto_season_end_unix: 1791277199,
            ..Default::default()
        };
        // Before the midpoint -> first battlepass end (Tue 2026-09-01 10:58:19).
        let before = battlepass_target(&cfg, dt(2026, 8, 17, 20, 0)).unwrap();
        assert_eq!(before, Utc.timestamp_opt(1788260299, 0).unwrap());
        // After the midpoint -> season end.
        let after = battlepass_target(&cfg, dt(2026, 9, 20, 0, 0)).unwrap();
        assert_eq!(after, Utc.timestamp_opt(1791277199, 0).unwrap());
    }

    #[test]
    fn battlepass_manual_override_wins() {
        use crate::settings::{SeasonConfig, UtcResetTime};
        let cfg = SeasonConfig {
            auto_season_start_unix: 1785761800,
            auto_season_end_unix: 1791277199,
            battlepass_reset: UtcResetTime {
                year: 2026,
                month: 9,
                day: 10,
                hour: 9,
                minute: 0,
            },
            ..Default::default()
        };
        assert_eq!(
            battlepass_target(&cfg, dt(2026, 8, 17, 20, 0)),
            Some(dt(2026, 9, 10, 9, 0))
        );
    }

    #[test]
    fn battlepass_falls_back_to_season_reset_without_span() {
        use crate::settings::{SeasonConfig, UtcResetTime};
        let cfg = SeasonConfig {
            reset: UtcResetTime {
                year: 2026,
                month: 10,
                day: 6,
                hour: 9,
                minute: 0,
            },
            ..Default::default()
        };
        assert_eq!(
            battlepass_target(&cfg, dt(2026, 8, 17, 20, 0)),
            Some(dt(2026, 10, 6, 9, 0))
        );
    }

    #[test]
    fn remaining_is_none_when_past() {
        let now = dt(2025, 6, 15, 12, 0);
        assert!(remaining(now, dt(2025, 6, 15, 11, 0)).is_none());
        assert!(remaining(now, now).is_none());
        assert!(remaining(now, dt(2025, 6, 15, 13, 0)).is_some());
    }

    #[test]
    fn daily_calendar_end_rolls_over_december() {
        // 23:59 UTC on the last day of the current month.
        assert_eq!(
            daily_calendar_end(dt(2025, 12, 20, 5, 0)),
            dt(2025, 12, 31, 23, 59)
        );
        assert_eq!(
            daily_calendar_end(dt(2025, 6, 15, 8, 0)),
            dt(2025, 6, 30, 23, 59)
        );
        // Leap-year February ends on the 29th.
        assert_eq!(
            daily_calendar_end(dt(2024, 2, 10, 10, 0)),
            dt(2024, 2, 29, 23, 59)
        );
    }

    #[test]
    fn final_window_boundaries() {
        let target = dt(2025, 7, 1, 0, 0);
        // Exactly 24h out -> inside a 24h window.
        assert!(within_final_window(dt(2025, 6, 30, 0, 0), target, 24));
        // 24h + 1m out -> outside.
        assert!(!within_final_window(dt(2025, 6, 29, 23, 59), target, 24));
        // Past reset -> outside.
        assert!(!within_final_window(dt(2025, 7, 1, 0, 1), target, 24));
    }

    #[test]
    fn format_examples() {
        assert_eq!(
            format_remaining(Duration::minutes(6 * 24 * 60 + 19 * 60)),
            "6d 19h"
        );
        assert_eq!(format_remaining(Duration::minutes(19 * 60 + 5)), "19h 5m");
        assert_eq!(format_remaining(Duration::minutes(5)), "5m");
        assert_eq!(format_remaining(Duration::seconds(30)), "<1m");
    }
}
