use jiff::tz::TimeZone;
use tg_bot_predlozhka::time::{day_start, format_utc, month_range, year_month};

fn moscow() -> TimeZone {
    TimeZone::get("Europe/Moscow").unwrap()
}
// 2026-03-10 20:59:59 UTC = 23:59:59 MSK; one second later is the next Moscow day.
const BEFORE_MIDNIGHT: i64 = 1_773_176_399;

#[test]
fn daily_limit_resets_at_moscow_midnight() {
    let tz = moscow();
    let start = day_start(&tz, BEFORE_MIDNIGHT).unwrap();
    assert_eq!(format_utc(start), "2026-03-09 21:00");
    assert_eq!(
        format_utc(day_start(&tz, BEFORE_MIDNIGHT + 1).unwrap()),
        "2026-03-10 21:00"
    );
}

#[test]
fn month_range_matches_python_moscow_offset() {
    // tests/test_time.py::test_month_range_moscow_offset
    let (start, end) = month_range(&moscow(), 2024, 3).unwrap();
    assert_eq!(format_utc(start), "2024-02-29 21:00");
    assert_eq!(format_utc(end), "2024-03-31 21:00");
}

#[test]
fn december_rolls_over_and_utc_is_identity() {
    let (_, end) = month_range(&moscow(), 2024, 12).unwrap();
    assert_eq!(format_utc(end), "2024-12-31 21:00");
    let (start, end) = month_range(&TimeZone::UTC, 2024, 2).unwrap();
    assert_eq!(
        (format_utc(start).as_str(), format_utc(end).as_str()),
        ("2024-02-01 00:00", "2024-03-01 00:00")
    );
}

#[test]
fn current_month_follows_local_calendar() {
    // 2026-08-31 21:30 UTC is already September in Moscow.
    let ts = 1_788_211_800;
    assert_eq!(format_utc(ts), "2026-08-31 21:30");
    assert_eq!(year_month(&moscow(), ts).unwrap(), (2026, 9));
    assert_eq!(year_month(&TimeZone::UTC, ts).unwrap(), (2026, 8));
}
