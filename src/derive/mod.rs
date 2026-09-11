//! Everything the API and UI need per local day, precomputed.
//!
//! "Day" here is always the local day, taken from the `secondsFromGMT` Arc records on every
//! sample and place, never the UTC day. An item that runs over local midnight belongs to both
//! days and is clipped into each; a flight can legitimately start and end at different offsets.

mod days;
mod items;

use anyhow::Result;
use jiff::civil::Date;
use jiff::tz::Offset;
use jiff::{Timestamp, ToSpan};
use rusqlite::Connection;

pub use days::recompute_days;
pub use items::derive_items;

/// The local calendar date of an instant at a given UTC offset, `YYYY-MM-DD`.
pub fn local_date(timestamp: Timestamp, offset_seconds: i32) -> String {
    offset(offset_seconds)
        .to_datetime(timestamp)
        .date()
        .to_string()
}

/// An offset outside ±26 h is a corrupt record, not something worth failing a bucket file over;
/// UTC is the least surprising stand-in.
pub fn offset(seconds: i32) -> Offset {
    Offset::from_seconds(seconds).unwrap_or(Offset::UTC)
}

/// Midnight starting `date`, as an instant at `offset_seconds`. Saturates at the ends of the
/// representable range, which only a date thousands of years out could reach.
pub fn day_start(date: Date, offset_seconds: i32) -> Timestamp {
    offset(offset_seconds)
        .to_timestamp(date.at(0, 0, 0, 0))
        .unwrap_or(if date.year() < 1970 {
            Timestamp::MIN
        } else {
            Timestamp::MAX
        })
}

/// Every date from `from` to `to` inclusive. Empty if they are the wrong way round.
pub fn dates_in_range(from: Date, to: Date) -> Vec<Date> {
    let mut dates = Vec::new();
    let mut current = from;
    while current <= to {
        dates.push(current);
        match current.checked_add(1.day()) {
            Ok(next) => current = next,
            Err(_) => break,
        }
    }
    dates
}

/// Recompute every item and every day from scratch. Backs `arches derive`, for when the
/// derivation rules change and the incremental path would keep stale rows around; a future
/// migration can call it too.
pub fn derive_all(conn: &mut Connection) -> Result<(usize, usize)> {
    let ids: Vec<String> = conn
        // Start order means the previous-item offset fallback usually finds a resolved neighbour.
        .prepare("SELECT id FROM items ORDER BY start_date")?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    let items = derive_items(conn, &ids)?;

    let days = recompute_days(conn, &all_dates(conn)?)?;
    Ok((items, days))
}

/// Every date that could hold a summary: the days items cover, the days samples fall on, and
/// the days that already have a row so an emptied day gets its row removed.
fn all_dates(conn: &Connection) -> Result<Vec<String>> {
    let mut dates: std::collections::BTreeSet<String> = conn
        .prepare(
            "SELECT DISTINCT local_date FROM samples WHERE local_date IS NOT NULL
             UNION SELECT date FROM day_summaries",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;

    let mut statement = conn.prepare(
        "SELECT DISTINCT local_start_date, local_end_date FROM items
         WHERE local_start_date IS NOT NULL AND local_end_date IS NOT NULL",
    )?;
    let spans = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for span in spans {
        let (from, to) = span?;
        // A visit can run for days, and every day in between needs a row of its own.
        let (Ok(from), Ok(to)) = (from.parse::<Date>(), to.parse::<Date>()) else {
            continue;
        };
        dates.extend(dates_in_range(from, to).into_iter().map(|d| d.to_string()));
    }
    Ok(dates.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn timestamp(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    #[test]
    fn local_date_follows_the_offset_not_utc() {
        let just_before_local_midnight = timestamp("2025-06-10T21:59:00Z");
        assert_eq!(local_date(just_before_local_midnight, 7200), "2025-06-10");
        assert_eq!(local_date(just_before_local_midnight, 0), "2025-06-10");

        let after_local_midnight = timestamp("2025-06-10T22:30:00Z");
        assert_eq!(local_date(after_local_midnight, 7200), "2025-06-11");
        assert_eq!(local_date(after_local_midnight, 0), "2025-06-10");
        // Honolulu is still the previous day.
        assert_eq!(local_date(after_local_midnight, -36000), "2025-06-10");
    }

    #[test]
    fn absurd_offsets_fall_back_to_utc() {
        assert_eq!(offset(999_999), Offset::UTC);
        assert_eq!(offset(7200).seconds(), 7200);
    }

    #[test]
    fn day_start_is_local_midnight() {
        let date: Date = "2025-06-10".parse().unwrap();
        assert_eq!(day_start(date, 7200), timestamp("2025-06-09T22:00:00Z"));
        assert_eq!(day_start(date, 0), timestamp("2025-06-10T00:00:00Z"));
    }

    #[test]
    fn dates_in_range_is_inclusive_and_ordered() {
        let from: Date = "2025-06-09".parse().unwrap();
        let to: Date = "2025-06-11".parse().unwrap();
        let dates: Vec<String> = dates_in_range(from, to)
            .into_iter()
            .map(|d| d.to_string())
            .collect();
        assert_eq!(dates, ["2025-06-09", "2025-06-10", "2025-06-11"]);
        assert_eq!(dates_in_range(to, from).len(), 0);
        assert_eq!(dates_in_range(from, from).len(), 1);
    }
}
