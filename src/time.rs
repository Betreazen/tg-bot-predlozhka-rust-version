//! All stored times are Unix seconds in UTC; calendar logic (the daily limit and
//! monthly statistics) uses the configured timezone, as in the Python bot.
use anyhow::Result;
use jiff::{Timestamp, civil::Date, tz::TimeZone};

pub fn now() -> i64 {
    Timestamp::now().as_second()
}

/// Start of the local calendar day containing `ts`.
pub fn day_start(tz: &TimeZone, ts: i64) -> Result<i64> {
    let local = Timestamp::from_second(ts)?.to_zoned(tz.clone());
    Ok(local.start_of_day()?.timestamp().as_second())
}

/// Half-open UTC range `[start, end)` of a local calendar month.
pub fn month_range(tz: &TimeZone, year: i16, month: i8) -> Result<(i64, i64)> {
    let first = Date::new(year, month, 1)?;
    let next = first.checked_add(jiff::Span::new().months(1))?;
    let start = first.to_zoned(tz.clone())?.timestamp().as_second();
    let end = next.to_zoned(tz.clone())?.timestamp().as_second();
    Ok((start, end))
}

pub fn year_month(tz: &TimeZone, ts: i64) -> Result<(i16, i8)> {
    let local = Timestamp::from_second(ts)?.to_zoned(tz.clone());
    Ok((local.year(), local.month()))
}

/// `%Y-%m-%d %H:%M` in UTC — the Python bot showed naive UTC times to admins.
pub fn format_utc(ts: i64) -> String {
    Timestamp::from_second(ts)
        .map(|t| t.strftime("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}
