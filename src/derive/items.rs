//! Per-item local offsets and dates.
//!
//! Items carry no timezone of their own; the offset has to come from the samples Arc attached
//! to them. Start and end are resolved separately because a flight legitimately starts in one
//! offset and ends in another.

use anyhow::Result;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension, params};

use super::local_date;

struct ItemRow {
    start_date: i64,
    end_date: i64,
    previous_item_id: Option<String>,
    next_item_id: Option<String>,
    /// The place's own offset, for a visit whose samples carry none.
    place_offset: Option<i32>,
}

/// Resolve `start_offset_seconds`, `end_offset_seconds` and the two local dates for `ids`.
/// Returns how many items were updated.
pub fn derive_items(conn: &mut Connection, ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let tx = conn.transaction()?;
    let mut updated = 0;
    {
        let mut load = tx.prepare(
            "SELECT i.start_date, i.end_date, i.previous_item_id, i.next_item_id,
                    p.seconds_from_gmt
             FROM items i LEFT JOIN places p ON p.id = i.visit_place_id
             WHERE i.id = ?",
        )?;
        // The first and last sample that has an offset at all: a sample recorded before Arc
        // captured timezones has none, and it should not veto the ones that do.
        let mut first_offset = tx.prepare(
            "SELECT seconds_from_gmt FROM samples
             WHERE timeline_item_id = ? AND seconds_from_gmt IS NOT NULL
             ORDER BY date LIMIT 1",
        )?;
        let mut last_offset = tx.prepare(
            "SELECT seconds_from_gmt FROM samples
             WHERE timeline_item_id = ? AND seconds_from_gmt IS NOT NULL
             ORDER BY date DESC LIMIT 1",
        )?;
        let mut neighbour =
            tx.prepare("SELECT start_offset_seconds, end_offset_seconds FROM items WHERE id = ?")?;
        let mut update = tx.prepare(
            "UPDATE items SET start_offset_seconds = ?, end_offset_seconds = ?,
                    local_start_date = ?, local_end_date = ?
             WHERE id = ?",
        )?;

        for id in ids {
            let Some(item) = load
                .query_row(params![id], |row| {
                    Ok(ItemRow {
                        start_date: row.get(0)?,
                        end_date: row.get(1)?,
                        previous_item_id: row.get(2)?,
                        next_item_id: row.get(3)?,
                        place_offset: row.get(4)?,
                    })
                })
                .optional()?
            else {
                // A sample can name an item whose month bucket has not been ingested yet.
                continue;
            };

            let from_samples = |statement: &mut rusqlite::Statement<'_>| -> Result<Option<i32>> {
                Ok(statement
                    .query_row(params![id], |row| row.get(0))
                    .optional()?)
            };
            let mut start = from_samples(&mut first_offset)?.or(item.place_offset);
            let mut end = from_samples(&mut last_offset)?.or(item.place_offset);

            if start.is_none() || end.is_none() {
                let mut sibling = |sibling_id: &Option<String>| -> Result<Option<(i32, i32)>> {
                    let Some(sibling_id) = sibling_id else {
                        return Ok(None);
                    };
                    Ok(neighbour
                        .query_row(params![sibling_id], |row| {
                            Ok((row.get::<_, Option<i32>>(0)?, row.get::<_, Option<i32>>(1)?))
                        })
                        .optional()?
                        .and_then(|(from, to)| Some((from?, to?))))
                };
                // Whatever was going on either side of a sampleless item is the best guess left.
                let fallback = sibling(&item.previous_item_id)?
                    .map(|(_, end)| end)
                    .or(sibling(&item.next_item_id)?.map(|(start, _)| start));
                if fallback.is_none() {
                    tracing::warn!(
                        item_id = %id,
                        "no offset from samples, place or neighbours; assuming UTC"
                    );
                }
                let fallback = fallback.unwrap_or(0);
                start = Some(start.unwrap_or(fallback));
                end = Some(end.unwrap_or(fallback));
            }

            let (start, end) = (start.unwrap_or(0), end.unwrap_or(0));
            let local_start = millis_to_local_date(item.start_date, start);
            let local_end = millis_to_local_date(item.end_date, end);
            updated += update.execute(params![start, end, local_start, local_end, id])?;
        }
    }
    tx.commit()?;
    Ok(updated)
}

fn millis_to_local_date(millis: i64, offset_seconds: i32) -> Option<String> {
    Timestamp::from_millisecond(millis)
        .ok()
        .map(|timestamp| local_date(timestamp, offset_seconds))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::db;

    pub fn millis(text: &str) -> i64 {
        text.parse::<Timestamp>().unwrap().as_millisecond()
    }

    /// Minimal item row; everything the derivation does not read stays NULL.
    pub fn insert_item(conn: &Connection, id: &str, is_visit: bool, start: &str, end: &str) {
        conn.execute(
            "INSERT INTO items (id, is_visit, start_date, end_date, last_saved)
             VALUES (?, ?, ?, ?, 0)",
            params![id, is_visit, millis(start), millis(end)],
        )
        .unwrap();
    }

    pub fn insert_sample(
        conn: &Connection,
        id: &str,
        item_id: &str,
        date: &str,
        offset: Option<i32>,
    ) {
        let local = offset.map(|offset| local_date(date.parse().unwrap(), offset));
        conn.execute(
            "INSERT INTO samples (id, date, last_saved, timeline_item_id, seconds_from_gmt,
                                  local_date)
             VALUES (?, ?, 0, ?, ?, ?)",
            params![id, millis(date), item_id, offset, local],
        )
        .unwrap();
    }

    pub fn insert_place(conn: &Connection, id: &str, offset: Option<i32>) {
        conn.execute(
            "INSERT INTO places (id, name, latitude, longitude, seconds_from_gmt, last_saved)
             VALUES (?, 'fixture', 51.05, 13.73, ?, 0)",
            params![id, offset],
        )
        .unwrap();
    }

    fn offsets(conn: &Connection, id: &str) -> (Option<i32>, Option<i32>, String, String) {
        conn.query_row(
            "SELECT start_offset_seconds, end_offset_seconds, local_start_date, local_end_date
             FROM items WHERE id = ?",
            params![id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .unwrap()
    }

    #[test]
    fn offsets_come_from_the_first_and_last_sample() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "trip",
            false,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_sample(&conn, "s1", "trip", "2025-06-10T06:00:00Z", Some(7200));
        insert_sample(&conn, "s2", "trip", "2025-06-10T06:30:00Z", Some(7200));
        insert_sample(&conn, "s3", "trip", "2025-06-10T07:00:00Z", Some(7200));

        derive_items(&mut conn, &["trip".into()]).unwrap();

        let (start, end, local_start, local_end) = offsets(&conn, "trip");
        assert_eq!((start, end), (Some(7200), Some(7200)));
        assert_eq!(
            (local_start.as_str(), local_end.as_str()),
            ("2025-06-10", "2025-06-10")
        );
    }

    #[test]
    fn samples_without_an_offset_do_not_veto_the_ones_that_have_one() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "trip",
            false,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_sample(&conn, "s1", "trip", "2025-06-10T06:00:00Z", None);
        insert_sample(&conn, "s2", "trip", "2025-06-10T06:30:00Z", Some(7200));
        insert_sample(&conn, "s3", "trip", "2025-06-10T07:00:00Z", None);

        derive_items(&mut conn, &["trip".into()]).unwrap();

        assert_eq!(offsets(&conn, "trip").0, Some(7200));
        assert_eq!(offsets(&conn, "trip").1, Some(7200));
    }

    /// A flight out of Europe: the samples at each end disagree, and both are right.
    #[test]
    fn a_flight_keeps_a_different_offset_at_each_end() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "flight",
            false,
            "2025-06-10T21:30:00Z",
            "2025-06-10T23:30:00Z",
        );
        insert_sample(&conn, "s1", "flight", "2025-06-10T21:30:00Z", Some(7200));
        insert_sample(&conn, "s2", "flight", "2025-06-10T23:30:00Z", Some(0));

        derive_items(&mut conn, &["flight".into()]).unwrap();

        let (start, end, local_start, local_end) = offsets(&conn, "flight");
        assert_eq!((start, end), (Some(7200), Some(0)));
        // 23:30 local CEST on the 10th, landing at 23:30 UTC still on the 10th.
        assert_eq!(local_start, "2025-06-10");
        assert_eq!(local_end, "2025-06-10");
    }

    #[test]
    fn a_sampleless_visit_falls_back_to_its_place() {
        let mut conn = db::open_in_memory().unwrap();
        insert_place(&conn, "place", Some(7200));
        insert_item(
            &conn,
            "visit",
            true,
            "2025-06-10T21:30:00Z",
            "2025-06-10T23:30:00Z",
        );
        conn.execute(
            "UPDATE items SET visit_place_id = 'place' WHERE id = 'visit'",
            [],
        )
        .unwrap();

        derive_items(&mut conn, &["visit".into()]).unwrap();

        let (start, end, local_start, local_end) = offsets(&conn, "visit");
        assert_eq!((start, end), (Some(7200), Some(7200)));
        assert_eq!(
            (local_start.as_str(), local_end.as_str()),
            ("2025-06-10", "2025-06-11")
        );
    }

    #[test]
    fn a_sampleless_item_falls_back_to_its_neighbours() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "before",
            false,
            "2025-06-10T05:00:00Z",
            "2025-06-10T06:00:00Z",
        );
        insert_sample(&conn, "s1", "before", "2025-06-10T05:30:00Z", Some(7200));
        insert_item(
            &conn,
            "gap",
            false,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_item(
            &conn,
            "after",
            false,
            "2025-06-10T07:00:00Z",
            "2025-06-10T08:00:00Z",
        );
        insert_sample(&conn, "s2", "after", "2025-06-10T07:30:00Z", Some(3600));
        conn.execute(
            "UPDATE items SET previous_item_id = 'before', next_item_id = 'after' WHERE id = 'gap'",
            [],
        )
        .unwrap();

        derive_items(&mut conn, &["before".into(), "gap".into(), "after".into()]).unwrap();

        // The previous item wins over the next one.
        assert_eq!(offsets(&conn, "gap").0, Some(7200));
        assert_eq!(offsets(&conn, "gap").1, Some(7200));
    }

    #[test]
    fn the_next_item_is_used_when_there_is_no_previous_one() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "gap",
            false,
            "2025-06-10T06:00:00Z",
            "2025-06-10T07:00:00Z",
        );
        insert_item(
            &conn,
            "after",
            false,
            "2025-06-10T07:00:00Z",
            "2025-06-10T08:00:00Z",
        );
        insert_sample(&conn, "s2", "after", "2025-06-10T07:30:00Z", Some(-18000));
        conn.execute(
            "UPDATE items SET next_item_id = 'after' WHERE id = 'gap'",
            [],
        )
        .unwrap();

        derive_items(&mut conn, &["after".into(), "gap".into()]).unwrap();

        assert_eq!(offsets(&conn, "gap").0, Some(-18000));
        assert_eq!(offsets(&conn, "gap").1, Some(-18000));
    }

    #[test]
    fn an_item_with_nothing_to_go_on_assumes_utc() {
        let mut conn = db::open_in_memory().unwrap();
        insert_item(
            &conn,
            "lonely",
            false,
            "2025-06-10T23:30:00Z",
            "2025-06-11T00:30:00Z",
        );

        derive_items(&mut conn, &["lonely".into()]).unwrap();

        let (start, end, local_start, local_end) = offsets(&conn, "lonely");
        assert_eq!((start, end), (Some(0), Some(0)));
        assert_eq!(
            (local_start.as_str(), local_end.as_str()),
            ("2025-06-10", "2025-06-11")
        );
    }

    #[test]
    fn unknown_ids_are_ignored() {
        let mut conn = db::open_in_memory().unwrap();
        assert_eq!(derive_items(&mut conn, &["nope".into()]).unwrap(), 0);
        assert_eq!(derive_items(&mut conn, &[]).unwrap(), 0);
    }
}
