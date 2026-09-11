//! Turning request strings into values and stored integers back into strings.
//!
//! The database stores unix milliseconds; the API only ever speaks RFC 3339. Dates in paths and
//! query strings are local `YYYY-MM-DD`, never a UTC day.

use jiff::Timestamp;
use jiff::civil::Date;

use super::error::{ApiError, ApiResult};

pub fn parse_date(raw: &str) -> ApiResult<Date> {
    raw.parse()
        .map_err(|_| ApiError::bad_request(format!("{raw:?} is not a YYYY-MM-DD date")))
}

pub fn parse_timestamp(raw: &str) -> ApiResult<Timestamp> {
    raw.parse()
        .map_err(|_| ApiError::bad_request(format!("{raw:?} is not an RFC 3339 timestamp")))
}

/// Today in the server's own zone. The API's default day range is relative to it, and a
/// machine serving a tailnet is in the same place as the person reading it.
pub fn today() -> Date {
    jiff::Zoned::now().date()
}

pub fn rfc3339(millis: i64) -> Option<String> {
    Timestamp::from_millisecond(millis)
        .ok()
        .map(|timestamp| timestamp.to_string())
}

/// A limit query parameter, defaulted and capped so a typo cannot ask for the whole table.
pub fn parse_limit(raw: Option<&str>, default: i64, max: i64) -> ApiResult<i64> {
    let Some(raw) = raw else { return Ok(default) };
    let limit: i64 = raw
        .parse()
        .map_err(|_| ApiError::bad_request(format!("{raw:?} is not a number")))?;
    if limit < 1 {
        return Err(ApiError::bad_request("limit must be at least 1"));
    }
    Ok(limit.min(max))
}

pub fn parse_f64(name: &str, raw: &str) -> ApiResult<f64> {
    let value: f64 = raw
        .parse()
        .map_err(|_| ApiError::bad_request(format!("{name} {raw:?} is not a number")))?;
    if !value.is_finite() {
        return Err(ApiError::bad_request(format!("{name} must be finite")));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_default_and_cap() {
        assert_eq!(parse_limit(None, 50, 500).unwrap(), 50);
        assert_eq!(parse_limit(Some("10"), 50, 500).unwrap(), 10);
        assert_eq!(parse_limit(Some("9000"), 50, 500).unwrap(), 500);
        assert!(parse_limit(Some("0"), 50, 500).is_err());
        assert!(parse_limit(Some("many"), 50, 500).is_err());
    }

    #[test]
    fn timestamps_render_as_rfc_3339() {
        assert_eq!(
            rfc3339(1749542400000).as_deref(),
            Some("2025-06-10T08:00:00Z")
        );
    }
}
