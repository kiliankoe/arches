//! A day as GPX 1.1: a waypoint per visit, a track per trip.

use anyhow::Result;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use jiff::civil::Date;
use rusqlite::Connection;

use super::convert::rfc3339;
use super::error::{ApiError, ApiResult};
use super::{AppState, days, model};

pub async fn day(state: &AppState, date: Date) -> ApiResult<Response> {
    let document = state
        .db
        .read(move |conn| {
            if !days::summary_exists(conn, &date)? {
                return Ok(None);
            }
            Ok(Some(render(conn, &date.to_string())?))
        })
        .await?;

    match document {
        Some(document) => {
            Ok(([(header::CONTENT_TYPE, "application/gpx+xml")], document).into_response())
        }
        None => Err(ApiError::not_found(format!("no summary for {date}"))),
    }
}

fn render(conn: &Connection, date: &str) -> Result<String> {
    let items = model::items_on(conn, date)?;
    let places = model::places_of(conn, &items)?;
    let mut samples = model::day_samples(conn, date)?;

    let mut gpx = String::from(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" creator="arches" xmlns="http://www.topografix.com/GPX/1/1">
"#,
    );
    gpx.push_str(&format!(
        "  <metadata><name>{}</name></metadata>\n",
        escape(date)
    ));

    for item in &items {
        let place = item.visit_place_id.as_deref().and_then(|id| places.get(id));
        if item.is_visit {
            let Some((latitude, longitude)) = item.visit_coordinates(place) else {
                continue;
            };
            let name = item
                .visit_custom_title
                .clone()
                .or_else(|| place.map(|place| place.name.clone()))
                .unwrap_or_else(|| "visit".to_string());
            gpx.push_str(&format!("  <wpt lat=\"{latitude}\" lon=\"{longitude}\">\n"));
            if let Some(time) = rfc3339(item.start_date) {
                gpx.push_str(&format!("    <time>{}</time>\n", escape(&time)));
            }
            gpx.push_str(&format!("    <name>{}</name>\n", escape(&name)));
            gpx.push_str("  </wpt>\n");
            continue;
        }

        let Some(samples) = samples.remove(&item.id) else {
            continue;
        };
        let activity = item
            .activity_type()
            .unwrap_or_else(|| "unknown".to_string());
        gpx.push_str("  <trk>\n");
        let name = format!(
            "{activity} {}",
            rfc3339(item.start_date).unwrap_or_default()
        );
        gpx.push_str(&format!("    <name>{}</name>\n", escape(name.trim())));
        gpx.push_str(&format!("    <type>{}</type>\n", escape(&activity)));
        gpx.push_str("    <trkseg>\n");
        for sample in &samples {
            let Some((latitude, longitude)) = sample.coordinates() else {
                continue;
            };
            gpx.push_str(&format!(
                "      <trkpt lat=\"{latitude}\" lon=\"{longitude}\">\n"
            ));
            if let Some(altitude) = sample.altitude {
                gpx.push_str(&format!("        <ele>{altitude}</ele>\n"));
            }
            if let Some(time) = rfc3339(sample.date) {
                gpx.push_str(&format!("        <time>{}</time>\n", escape(&time)));
            }
            gpx.push_str("      </trkpt>\n");
        }
        gpx.push_str("    </trkseg>\n  </trk>\n");
    }

    gpx.push_str("</gpx>\n");
    Ok(gpx)
}

/// Place names come straight from Arc and contain whatever the user typed, so everything that
/// goes into the document is escaped. A whole XML crate would be a lot for five characters.
fn escape(raw: &str) -> String {
    let mut escaped = String::with_capacity(raw.len());
    for character in raw.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_everything_xml_cares_about() {
        assert_eq!(escape("coffee & cake"), "coffee &amp; cake");
        assert_eq!(
            escape(r#"<a href="x">'y'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&apos;y&apos;&lt;/a&gt;"
        );
        assert_eq!(escape("Hauptbahnhof"), "Hauptbahnhof");
    }
}
