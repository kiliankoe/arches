//! Deterministic record ids for GPX rows.
//!
//! GPX carries no ids, but ingest has to be an upsert: re-exporting the archive, or re-running
//! over an unchanged file, must land on the same rows rather than duplicate every day of
//! history. So an id is a hash of what identifies the record in the file, rendered in the UUID
//! shape the rest of the database uses.
//!
//! The hash is FNV-1a rather than `std::hash`, whose `DefaultHasher` is explicitly not stable
//! across Rust releases: an id that changed with the compiler would fork the whole archive.

/// FNV-1a 128-bit, from the reference parameters.
const OFFSET_BASIS: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;

/// A stable id for `(source, kind, key)`, e.g. the file name and element index of a track.
/// `kind` keeps the trip, its fixes and the place derived from the same string apart.
pub fn deterministic_id(source: &str, kind: &str, key: &str) -> String {
    let mut hash = OFFSET_BASIS;
    // The separator means `("a", "bc")` and `("ab", "c")` cannot collide.
    for part in [
        source.as_bytes(),
        b"\0",
        kind.as_bytes(),
        b"\0",
        key.as_bytes(),
    ] {
        for byte in part {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    }
    format_uuid(hash)
}

/// The 128 bits as a version 8 (custom) UUID, so a GPX id is visibly the same shape as the
/// UUIDs Arc's own records carry.
fn format_uuid(hash: u128) -> String {
    let mut bytes = hash.to_be_bytes();
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let hex: String = bytes.iter().map(|byte| format!("{byte:02X}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_uuid_shaped_and_stable() {
        let id = deterministic_id("quantified-map-gpx", "trip", "2016-02-18.gpx#0");

        // Pinned: changing the hash would fork every id in an existing database.
        assert_eq!(id, "7F325572-9C54-8496-9BD5-E4A71FC10F65");
        assert_eq!(id.len(), 36);
        assert_eq!(
            id.chars().filter(|c| *c == '-').count(),
            4,
            "not a UUID shape: {id}"
        );
        // Version 8, RFC 4122 variant.
        assert_eq!(&id[14..15], "8");
        assert!(["8", "9", "A", "B"].contains(&&id[19..20]));
    }

    #[test]
    fn every_part_changes_the_id() {
        let id = deterministic_id("quantified-map-gpx", "trip", "2016-02-18.gpx#0");
        for other in [
            deterministic_id("other", "trip", "2016-02-18.gpx#0"),
            deterministic_id("quantified-map-gpx", "sample", "2016-02-18.gpx#0"),
            deterministic_id("quantified-map-gpx", "trip", "2016-02-18.gpx#1"),
            // The separator keeps a shifted split from colliding.
            deterministic_id("quantified-map-gpx", "tri", "p2016-02-18.gpx#0"),
        ] {
            assert_ne!(id, other);
        }
        assert_eq!(
            id,
            deterministic_id("quantified-map-gpx", "trip", "2016-02-18.gpx#0")
        );
    }
}
