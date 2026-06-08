//! Automatic frontlight calculations based on sunrise and sunset.
//!
//! The logic in this module keeps brightness and warmth aligned with the
//! current solar day for a given location.

use crate::geolocation::Coordinates;

use super::{LightLevel, LightLevels};
use chrono::{DateTime, Local, NaiveDateTime, Timelike};

const MINUTES_PER_DAY: f64 = 24.0 * 60.0;

const WARMTH_TRANSITION_HOURS: f64 = 1.5;

/// Computes the frontlight levels that should be active at the given time.
///
/// Brightness switches between `current_intensity` during daylight hours and
/// `night_brightness` while the sun is down. Warmth ramps over a fixed
/// transition window before sunrise and before sunset, reaching fully cool at
/// sunrise and fully warm at sunset.
pub fn compute_auto_frontlight_levels(
    now: DateTime<Local>,
    coordinates: Coordinates,
    night_brightness: LightLevel,
    current_intensity: LightLevel,
) -> LightLevels {
    let today = now.date_naive();
    let coords: sunrise::Coordinates = coordinates.into();
    let solar_day = sunrise::SolarDay::new(coords, today);
    let sunrise_utc = solar_day.event_time(sunrise::SolarEvent::Sunrise);
    let sunset_utc = solar_day.event_time(sunrise::SolarEvent::Sunset);

    let minutes_since_midnight =
        |dt: NaiveDateTime| -> f64 { (dt.hour() as f64 * 60.0) + dt.minute() as f64 };

    let now_min = (now.hour() as f64 * 60.0) + now.minute() as f64;
    let offset = *now.offset();
    let sr_local = sunrise_utc.with_timezone(&offset).naive_local();
    let ss_local = sunset_utc.with_timezone(&offset).naive_local();
    let sr_min = minutes_since_midnight(sr_local);
    let ss_min = minutes_since_midnight(ss_local);
    let transition_min = WARMTH_TRANSITION_HOURS * 60.0;

    let sun_is_down = now_min < sr_min || now_min > ss_min;

    let intensity = if sun_is_down {
        night_brightness
    } else {
        current_intensity
    };

    let evening_ramp_start = ss_min - transition_min;
    let evening_ramp_end = ss_min;
    let morning_ramp_start = sr_min - transition_min;
    let morning_ramp_end = sr_min;

    let normalized_minute = |minute: f64| minute.rem_euclid(MINUTES_PER_DAY);
    let minute_in_wrapped_range = |minute: f64, start: f64, end: f64| {
        let minute = normalized_minute(minute);
        let start = normalized_minute(start);
        let end = normalized_minute(end);

        if start <= end {
            minute >= start && minute < end
        } else {
            minute >= start || minute < end
        }
    };
    let wrapped_range_progress = |minute: f64, start: f64, end: f64| {
        let span = (end - start).rem_euclid(MINUTES_PER_DAY);
        let elapsed = (minute - start).rem_euclid(MINUTES_PER_DAY);
        elapsed / span
    };

    let warmth_fraction: f32 =
        if minute_in_wrapped_range(now_min, evening_ramp_end, morning_ramp_start) {
            1.0
        } else if minute_in_wrapped_range(now_min, morning_ramp_end, evening_ramp_start) {
            0.0
        } else if minute_in_wrapped_range(now_min, evening_ramp_start, evening_ramp_end) {
            wrapped_range_progress(now_min, evening_ramp_start, evening_ramp_end) as f32
        } else {
            1.0 - wrapped_range_progress(now_min, morning_ramp_start, morning_ramp_end) as f32
        };

    LightLevels {
        intensity,
        warmth: LightLevel::from_fraction(warmth_fraction),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn london() -> Coordinates {
        Coordinates::new(51.5074, -0.1278).unwrap()
    }

    fn tromso() -> Coordinates {
        Coordinates::new(69.6492, 18.9553).unwrap()
    }

    fn make_dt(date: &str, time: &str) -> DateTime<Local> {
        let naive =
            chrono::NaiveDateTime::parse_from_str(&format!("{date} {time}"), "%Y-%m-%d %H:%M:%S")
                .unwrap();
        Local.from_local_datetime(&naive).single().unwrap()
    }

    fn compute_expected_sun_times(date: chrono::NaiveDate, coords: Coordinates) -> (f64, f64) {
        let day = sunrise::SolarDay::new(coords.into(), date);
        let sr = day
            .event_time(sunrise::SolarEvent::Sunrise)
            .with_timezone(&Local)
            .naive_local();
        let ss = day
            .event_time(sunrise::SolarEvent::Sunset)
            .with_timezone(&Local)
            .naive_local();
        (
            (sr.hour() as f64 * 60.0) + sr.minute() as f64,
            (ss.hour() as f64 * 60.0) + ss.minute() as f64,
        )
    }

    #[test]
    fn night_brightness_is_applied_when_sun_is_down() {
        let midnight = make_dt("2025-06-21", "00:00:00");
        let levels = compute_auto_frontlight_levels(midnight, london(), 10.0.into(), 50.0.into());
        assert_eq!(
            levels.intensity, 10.0,
            "night brightness should be night_brightness"
        );
    }

    #[test]
    fn day_brightness_preserves_current_intensity() {
        let noon = make_dt("2025-06-21", "12:00:00");
        let levels = compute_auto_frontlight_levels(noon, london(), 10.0.into(), 50.0.into());
        assert_eq!(
            levels.intensity, 50.0,
            "day brightness should be current_intensity"
        );
    }

    #[test]
    fn warmth_is_zero_at_sunrise() {
        let sr_utc = sunrise::SolarDay::new(
            london().into(),
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
        )
        .event_time(sunrise::SolarEvent::Sunrise);
        let sr_local = sr_utc.with_timezone(&Local);
        let levels = compute_auto_frontlight_levels(sr_local, london(), 10.0.into(), 50.0.into());
        assert!(
            levels.warmth < 1.0,
            "at sunrise warmth should be ~0, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_one_hundred_at_sunset() {
        let ss_utc = sunrise::SolarDay::new(
            london().into(),
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
        )
        .event_time(sunrise::SolarEvent::Sunset);
        let ss_local = ss_utc.with_timezone(&Local);
        let levels = compute_auto_frontlight_levels(ss_local, london(), 10.0.into(), 50.0.into());
        assert!(
            (levels.warmth - 100.0).abs() < 1.0,
            "at sunset warmth should be ~100, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_zero_during_middle_of_day() {
        let noon = make_dt("2025-06-21", "12:00:00");
        let levels = compute_auto_frontlight_levels(noon, london(), 10.0.into(), 50.0.into());
        assert!(
            levels.warmth < 1.0,
            "midday warmth should be ~0, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_is_one_hundred_during_middle_of_night() {
        let midnight = make_dt("2025-06-21", "00:00:00");
        let levels = compute_auto_frontlight_levels(midnight, london(), 10.0.into(), 50.0.into());
        assert!(
            (levels.warmth - 100.0).abs() < 1.0,
            "midnight warmth should be ~100, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_ramps_from_zero_to_one_hundred_in_evening_transition() {
        let (_, ss) = compute_expected_sun_times(
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
            london(),
        );
        let transition = 90.0;

        let ramp_start = (ss - transition) as i64;
        let h = ramp_start / 60;
        let m = ramp_start % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert!(
            levels.warmth < 2.0,
            "evening ramp start: warmth should be ~0, got {}",
            levels.warmth
        );

        let midpoint = (ss - transition / 2.0) as i64;
        let h = midpoint / 60;
        let m = midpoint % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert!(
            (levels.warmth - 50.0).abs() < 6.0,
            "evening ramp midpoint: warmth should be ~50, got {}",
            levels.warmth
        );
    }

    #[test]
    fn warmth_ramps_from_one_hundred_to_zero_in_morning_transition() {
        let (sr, _) = compute_expected_sun_times(
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
            london(),
        );
        let transition = 90.0;

        let ramp_start = (sr - transition) as i64;
        let h = ramp_start / 60;
        let m = ramp_start % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert!(
            (levels.warmth - 100.0).abs() < 2.0,
            "morning ramp start: warmth should be ~100, got {}",
            levels.warmth
        );

        let midpoint = (sr - transition / 2.0) as i64;
        let h = midpoint / 60;
        let m = midpoint % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert!(
            (levels.warmth - 50.0).abs() < 6.0,
            "morning ramp midpoint: warmth should be ~50, got {}",
            levels.warmth
        );
    }

    #[test]
    fn evening_brightness_is_night_level() {
        let (_, ss) = compute_expected_sun_times(
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
            london(),
        );
        let post_sunset_min = ss + 30.0;
        let h = post_sunset_min as i64 / 60;
        let m = post_sunset_min as i64 % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert_eq!(
            levels.intensity, 10.0,
            "post-sunset brightness should be night_brightness"
        );
    }

    #[test]
    fn morning_brightness_is_night_level_before_sunrise() {
        let (sr, _) = compute_expected_sun_times(
            chrono::NaiveDate::from_ymd_opt(2025, 6, 21).unwrap(),
            london(),
        );
        let pre_sunrise_min = sr - 120.0;
        let h = pre_sunrise_min as i64 / 60;
        let m = pre_sunrise_min as i64 % 60;
        let t = make_dt("2025-06-21", &format!("{h:02}:{m:02}:00"));
        let levels = compute_auto_frontlight_levels(t, london(), 10.0.into(), 50.0.into());
        assert_eq!(
            levels.intensity, 10.0,
            "pre-sunrise brightness should be night_brightness"
        );
    }

    #[test]
    fn warmth_stays_continuous_across_midnight_when_morning_ramp_wraps() {
        let coordinates = tromso();
        let before_midnight = make_dt("2025-05-15", "23:59:00");
        let after_midnight = make_dt("2025-05-16", "00:00:00");

        let before_levels =
            compute_auto_frontlight_levels(before_midnight, coordinates, 10.0.into(), 50.0.into());
        let after_levels =
            compute_auto_frontlight_levels(after_midnight, coordinates, 10.0.into(), 50.0.into());

        assert!(
            (f32::from(before_levels.warmth) - f32::from(after_levels.warmth)).abs() < 4.0,
            "warmth should stay continuous across midnight, got {} then {}",
            before_levels.warmth,
            after_levels.warmth
        );
    }
}
