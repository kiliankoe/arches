//! Whether the user has reviewed an item yet.
//!
//! Arc records its own guesses and the user's corrections in different columns for visits and
//! trips, so the two flags are defined once here and read the same way by the derivation and
//! by the API. Consumers use them to decide whether a day can be relied on yet.

use rusqlite::Row;

/// The columns [`Confirmation::from_row`] expects, in order, for splicing into a SELECT.
pub const COLUMNS: &str = "is_visit, visit_confirmed_place, visit_uncertain_place,
     trip_confirmed_activity_type, trip_uncertain_activity_type";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confirmation {
    pub confirmed: bool,
    pub uncertain: bool,
}

impl Confirmation {
    /// A visit is confirmed when the user picked its place; a trip when it has a confirmed
    /// activity type at all, which Arc leaves NULL until someone says so.
    pub fn new(
        is_visit: bool,
        visit_confirmed_place: Option<i64>,
        visit_uncertain_place: Option<i64>,
        trip_confirmed_activity_type: Option<i64>,
        trip_uncertain_activity_type: Option<i64>,
    ) -> Self {
        if is_visit {
            Self {
                confirmed: visit_confirmed_place == Some(1),
                uncertain: visit_uncertain_place == Some(1),
            }
        } else {
            Self {
                confirmed: trip_confirmed_activity_type.is_some(),
                uncertain: trip_uncertain_activity_type == Some(1),
            }
        }
    }

    /// Reads the five [`COLUMNS`] starting at `first`.
    pub fn from_row(row: &Row<'_>, first: usize) -> rusqlite::Result<Self> {
        Ok(Self::new(
            row.get(first)?,
            row.get(first + 1)?,
            row.get(first + 2)?,
            row.get(first + 3)?,
            row.get(first + 4)?,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_visit_reads_only_its_own_columns() {
        let confirmed = Confirmation::new(true, Some(1), Some(0), None, None);
        assert_eq!(
            confirmed,
            Confirmation {
                confirmed: true,
                uncertain: false
            }
        );
        // A trip's confirmed type sitting on a visit row means nothing.
        let unconfirmed = Confirmation::new(true, Some(0), Some(1), Some(24), Some(1));
        assert_eq!(
            unconfirmed,
            Confirmation {
                confirmed: false,
                uncertain: true
            }
        );
        assert!(!Confirmation::new(true, None, None, None, None).confirmed);
    }

    #[test]
    fn a_trip_is_confirmed_by_having_any_activity_type() {
        assert!(Confirmation::new(false, None, None, Some(24), Some(0)).confirmed);
        // Even an activity type arches does not know the name of is a user decision.
        assert!(Confirmation::new(false, None, None, Some(99), Some(1)).confirmed);
        let unreviewed = Confirmation::new(false, Some(1), Some(0), None, Some(1));
        assert_eq!(
            unreviewed,
            Confirmation {
                confirmed: false,
                uncertain: true
            }
        );
    }
}
