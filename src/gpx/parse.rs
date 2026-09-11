//! The streaming GPX reader.
//!
//! quick-xml pull parsing rather than a document tree: a day of one-second fixes is a few
//! megabytes of XML, and only the typed elements above are ever kept. Unknown elements and
//! attributes are ignored rather than rejected, since the files are an archive nobody will
//! regenerate to suit us.

use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use jiff::Timestamp;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use super::{Document, Element, Point, Track, Waypoint};

/// GPX files run to a few megabytes; the default 8 KiB buffer would refill hundreds of times.
const READ_BUFFER: usize = 64 * 1024;

/// Every track and waypoint in the file, in document order.
pub fn read(path: &Path) -> Result<Document> {
    let mut reader = open(path)?;
    let mut buf = Vec::new();
    let mut text = Vec::new();

    let mut creator = None;
    let mut elements = Vec::new();
    let mut track: Option<Track> = None;
    let mut point: Option<Point> = None;
    let mut waypoint: Option<Waypoint> = None;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .with_context(|| format!("parsing {}", path.display()))?;
        match event {
            Event::Eof => break,
            Event::Start(start) => match start.local_name().as_ref() {
                "gpx" => creator = attribute(&start, "creator")?,
                "wpt" => waypoint = waypoint_at(&start)?,
                "trk" => track = Some(Track::default()),
                "trkpt" => point = point_at(&start)?,
                name @ ("time" | "name" | "type" | "ele") => {
                    let value = read_text(&mut reader, &start, &mut text)?;
                    // Innermost container wins: a `<name>` inside a `<trkpt>` is not the track's.
                    match (name, &mut point, &mut waypoint, &mut track) {
                        ("time", Some(point), _, _) => point.time = parse_time(&value, path),
                        ("ele", Some(point), _, _) => point.altitude = value.parse().ok(),
                        (_, Some(_), _, _) => {}
                        ("time", _, Some(waypoint), _) => waypoint.time = parse_time(&value, path),
                        ("name", _, Some(waypoint), _) => waypoint.name = Some(value),
                        (_, _, Some(_), _) => {}
                        ("name", _, _, Some(track)) => track.name = Some(value),
                        ("type", _, _, Some(track)) => track.kind = Some(value),
                        _ => {}
                    }
                }
                _ => {}
            },
            Event::Empty(start) => match start.local_name().as_ref() {
                "gpx" => creator = attribute(&start, "creator")?,
                "wpt" => {
                    if let Some(waypoint) = waypoint_at(&start)? {
                        elements.push(Element::Waypoint(waypoint));
                    }
                }
                "trkpt" => {
                    if let Some(point) = point_at(&start)?
                        && let Some(track) = track.as_mut()
                    {
                        track.points.push(point);
                    }
                }
                _ => {}
            },
            Event::End(end) => match end.local_name().as_ref() {
                "trkpt" => {
                    if let Some(point) = point.take()
                        && let Some(track) = track.as_mut()
                    {
                        track.points.push(point);
                    }
                }
                "wpt" => {
                    if let Some(waypoint) = waypoint.take() {
                        elements.push(Element::Waypoint(waypoint));
                    }
                }
                "trk" => {
                    if let Some(track) = track.take() {
                        elements.push(Element::Track(track));
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }

    Ok(Document { creator, elements })
}

/// The `creator` attribute alone, read without parsing the body: it is the `source` every row
/// from this directory is stamped with, and one file answers for the whole directory.
pub fn creator_of(path: &Path) -> Result<Option<String>> {
    let mut reader = open(path)?;
    let mut buf = Vec::new();
    loop {
        let event = reader
            .read_event_into(&mut buf)
            .with_context(|| format!("parsing {}", path.display()))?;
        match event {
            Event::Eof => return Ok(None),
            Event::Start(start) | Event::Empty(start) if start.local_name().as_ref() == "gpx" => {
                return attribute(&start, "creator");
            }
            _ => {}
        }
        buf.clear();
    }
}

/// When the first element of `path` happens, stopping as soon as that is known.
///
/// A visit ends when the next thing in the stream starts, and the stream runs across files, so
/// the last visit of a file has to look into the next one. `skip` is the visit's own name and
/// start: a visit spanning UTC midnight is repeated verbatim at the head of the next file, and
/// that repetition is the same visit, not its successor.
pub fn next_element_time(
    path: &Path,
    skip: Option<(&str, Timestamp)>,
) -> Result<Option<Timestamp>> {
    let mut reader = open(path)?;
    let mut buf = Vec::new();
    let mut text = Vec::new();
    let mut waypoint: Option<Waypoint> = None;
    let mut in_point = false;

    loop {
        let event = reader
            .read_event_into(&mut buf)
            .with_context(|| format!("parsing {}", path.display()))?;
        match event {
            Event::Eof => return Ok(None),
            Event::Start(start) => match start.local_name().as_ref() {
                "wpt" => waypoint = waypoint_at(&start)?,
                "trkpt" => in_point = true,
                "time" => {
                    let value = read_text(&mut reader, &start, &mut text)?;
                    let time = parse_time(&value, path);
                    match (&mut waypoint, in_point) {
                        // A track begins at its first fix, so there is nothing else to wait for.
                        (_, true) if time.is_some() => return Ok(time),
                        (Some(waypoint), false) => waypoint.time = time,
                        _ => {}
                    }
                }
                "name" => {
                    let value = read_text(&mut reader, &start, &mut text)?;
                    if let Some(waypoint) = &mut waypoint {
                        waypoint.name = Some(value);
                    }
                }
                _ => {}
            },
            Event::End(end) => match end.local_name().as_ref() {
                "trkpt" => in_point = false,
                "wpt" => {
                    if let Some(waypoint) = waypoint.take()
                        && !repeats(&waypoint, skip)
                        && waypoint.time.is_some()
                    {
                        return Ok(waypoint.time);
                    }
                }
                _ => {}
            },
            _ => {}
        }
        buf.clear();
    }
}

/// Whether this waypoint is the midnight repetition of the visit described by `skip`.
fn repeats(waypoint: &Waypoint, skip: Option<(&str, Timestamp)>) -> bool {
    let Some((name, start)) = skip else {
        return false;
    };
    waypoint.time == Some(start) && waypoint.name.as_deref().unwrap_or_default() == name
}

fn open(path: &Path) -> Result<Reader<BufReader<File>>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(Reader::from_reader(BufReader::with_capacity(
        READ_BUFFER,
        file,
    )))
}

/// The text content of a leaf element, entity references resolved. Consumes up to and including
/// the element's end tag.
fn read_text<R: BufRead>(
    reader: &mut Reader<R>,
    start: &BytesStart<'_>,
    buf: &mut Vec<u8>,
) -> Result<String> {
    let end = start.to_end().into_owned();
    buf.clear();
    let raw = reader.read_text_into(end.name(), buf)?;
    Ok(unescape(&raw)?.trim().to_string())
}

fn attribute(start: &BytesStart<'_>, key: &str) -> Result<Option<String>> {
    for attribute in start.attributes() {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == key {
            return Ok(Some(
                attribute
                    .normalized_value(XmlVersion::Explicit1_0)?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

/// `lat` and `lon`, which GPX requires on both `<wpt>` and `<trkpt>`. An element missing either
/// is not a position and is dropped.
fn coordinates(start: &BytesStart<'_>) -> Result<Option<(f64, f64)>> {
    let number = |raw: Option<String>| raw.and_then(|raw| raw.trim().parse::<f64>().ok());
    let latitude = number(attribute(start, "lat")?);
    let longitude = number(attribute(start, "lon")?);
    Ok(latitude.zip(longitude))
}

fn waypoint_at(start: &BytesStart<'_>) -> Result<Option<Waypoint>> {
    Ok(coordinates(start)?.map(|(latitude, longitude)| Waypoint {
        latitude,
        longitude,
        time: None,
        name: None,
    }))
}

fn point_at(start: &BytesStart<'_>) -> Result<Option<Point>> {
    Ok(coordinates(start)?.map(|(latitude, longitude)| Point {
        latitude,
        longitude,
        altitude: None,
        time: None,
    }))
}

/// All times in these files are UTC with a `Z`. One unreadable timestamp costs its fix, not the
/// day around it.
fn parse_time(value: &str, path: &Path) -> Option<Timestamp> {
    match value.parse() {
        Ok(time) => Some(time),
        Err(error) => {
            tracing::warn!(path = %path.display(), value, %error, "unparseable GPX timestamp");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write(contents: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("2016-02-18.gpx");
        let mut file = File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        (temp, path)
    }

    fn parse(contents: &str) -> Document {
        let (_temp, path) = write(contents);
        read(&path).unwrap()
    }

    fn time(text: &str) -> Timestamp {
        text.parse().unwrap()
    }

    const DAY: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<gpx version="1.1" xmlns="http://www.topografix.com/GPX/1/1" creator="quantified-map-gpx">
  <trk>
    <name>Walking</name>
    <type>walking</type>
    <trkseg>
      <trkpt lat="51.0403" lon="13.7320">
        <ele>113.5</ele>
        <time>2016-02-18T08:00:00Z</time>
      </trkpt>
      <trkpt lat="51.0499" lon="13.7333">
        <time>2016-02-18T08:20:00Z</time>
      </trkpt>
    </trkseg>
  </trk>
  <wpt lat="51.0526" lon="13.7380">
    <time>2016-02-18T08:20:00Z</time>
    <name>coffee &amp; cake</name>
  </wpt>
  <somethingNew foo="bar">
    <time>2016-02-18T09:00:00Z</time>
  </somethingNew>
  <wpt lat="51.0380" lon="13.7300">
    <time>2016-02-18T10:00:00Z</time>
    <name>Unnamed Visit</name>
  </wpt>
</gpx>
"#;

    #[test]
    fn reads_tracks_and_waypoints_in_document_order() {
        let document = parse(DAY);

        assert_eq!(document.creator.as_deref(), Some("quantified-map-gpx"));
        assert_eq!(document.elements.len(), 3);

        let Element::Track(track) = &document.elements[0] else {
            panic!("first element is not a track: {:?}", document.elements[0]);
        };
        assert_eq!(track.name.as_deref(), Some("Walking"));
        assert_eq!(track.kind.as_deref(), Some("walking"));
        assert_eq!(track.points.len(), 2);
        assert_eq!(track.points[0].latitude, 51.0403);
        assert_eq!(track.points[0].longitude, 13.7320);
        assert_eq!(track.points[0].altitude, Some(113.5));
        assert_eq!(track.points[0].time, Some(time("2016-02-18T08:00:00Z")));
        // Altitude is per fix, not per track: only the older sources recorded it.
        assert_eq!(track.points[1].altitude, None);
        assert_eq!(
            document.elements[0].time(),
            Some(time("2016-02-18T08:00:00Z"))
        );
    }

    /// An entity in a place name has to arrive decoded, and an element nobody knows must not
    /// take its `<time>` with it.
    #[test]
    fn waypoints_decode_entities_and_unknown_elements_are_ignored() {
        let document = parse(DAY);

        let Element::Waypoint(waypoint) = &document.elements[1] else {
            panic!("second element is not a waypoint");
        };
        assert_eq!(waypoint.name.as_deref(), Some("coffee & cake"));
        assert_eq!(waypoint.latitude, 51.0526);
        assert_eq!(waypoint.time, Some(time("2016-02-18T08:20:00Z")));

        let Element::Waypoint(unnamed) = &document.elements[2] else {
            panic!("third element is not a waypoint");
        };
        assert_eq!(unnamed.name.as_deref(), Some("Unnamed Visit"));
        assert_eq!(unnamed.time, Some(time("2016-02-18T10:00:00Z")));
    }

    #[test]
    fn self_closing_and_empty_elements_parse() {
        let document = parse(
            r#"<gpx creator="x">
                 <wpt lat="51.05" lon="13.73"/>
                 <trk><trkseg><trkpt lat="51.06" lon="13.74"/></trkseg></trk>
               </gpx>"#,
        );

        assert_eq!(document.elements.len(), 2);
        let Element::Track(track) = &document.elements[1] else {
            panic!("not a track");
        };
        assert_eq!(track.points.len(), 1);
        assert_eq!(track.points[0].time, None);
        assert_eq!(document.elements[1].time(), None);
    }

    #[test]
    fn a_fix_without_coordinates_is_dropped_and_the_rest_survives() {
        let document = parse(
            r#"<gpx creator="x"><trk><trkseg>
                 <trkpt lon="13.74"><time>2016-02-18T08:00:00Z</time></trkpt>
                 <trkpt lat="51.06" lon="13.74"><time>2016-02-18T08:01:00Z</time></trkpt>
               </trkseg></trk></gpx>"#,
        );

        let Element::Track(track) = &document.elements[0] else {
            panic!("not a track");
        };
        assert_eq!(track.points.len(), 1);
        assert_eq!(track.points[0].time, Some(time("2016-02-18T08:01:00Z")));
    }

    #[test]
    fn malformed_xml_is_an_error() {
        let (_temp, path) = write("<gpx creator=\"x\"><trk><name>oops</trk></gpx>");
        assert!(read(&path).is_err());
    }

    #[test]
    fn creator_is_read_without_the_body() {
        let (_temp, path) = write(DAY);
        assert_eq!(
            creator_of(&path).unwrap().as_deref(),
            Some("quantified-map-gpx")
        );

        let (_temp, empty) = write("<gpx version=\"1.1\"></gpx>");
        assert_eq!(creator_of(&empty).unwrap(), None);
    }

    #[test]
    fn next_element_time_is_the_first_element_that_is_not_the_same_visit() {
        let (_temp, path) = write(DAY);

        assert_eq!(
            next_element_time(&path, None).unwrap(),
            Some(time("2016-02-18T08:00:00Z"))
        );

        // A visit repeated at the head of the next file is the same visit, so the answer is
        // what comes after it.
        let (_temp, midnight) = write(
            r#"<gpx creator="x">
                 <wpt lat="51.05" lon="13.73">
                   <time>2016-02-17T22:00:00Z</time>
                   <name>4.1 WG</name>
                 </wpt>
                 <trk><trkseg><trkpt lat="51.06" lon="13.74">
                   <time>2016-02-18T07:30:00Z</time>
                 </trkpt></trkseg></trk>
               </gpx>"#,
        );
        let start = time("2016-02-17T22:00:00Z");
        assert_eq!(
            next_element_time(&midnight, Some(("4.1 WG", start))).unwrap(),
            Some(time("2016-02-18T07:30:00Z"))
        );
        // A different visit at the same instant is not the same visit.
        assert_eq!(
            next_element_time(&midnight, Some(("Elsewhere", start))).unwrap(),
            Some(start)
        );
        assert_eq!(next_element_time(&midnight, None).unwrap(), Some(start));
    }
}
