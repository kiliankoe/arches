//! Notable events, derived from data arches already has.
//!
//! A highlight is one line in a feed or a journal: the first day in a country, the first day in
//! a town, a flight, the longest walk of a range. Nothing new is recorded for them; they are a
//! reading of `day_summaries` and `items`, computed per request.
//!
//! This module is pure on purpose: it takes plain rows and returns the events, so the rules
//! about what counts as a first time and which trip wins can be tested without a database or
//! an HTTP request. The SQL lives in `api/highlights.rs`.
//!
//! Every highlight carries `confirmed`, which says whether the user has reviewed the data it
//! was derived from in Arc. It is the consumer's job to decide what to do with an unconfirmed
//! event: a public feed should drop it, a private journal may well show it greyed out.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::Serialize;

use crate::arc::enums::ActivityType;

/// Local `YYYY-MM-DD` dates are compared as strings throughout: they sort chronologically, and
/// no arithmetic happens here that would need a real date type.
type Day = str;

/// How far past a non-visit neighbour a flight looks for the place it left from or arrived at.
/// A flight is usually bracketed by visits, but a walk through the terminal or a short transfer
/// ride in between is normal; anything further away is not the airport any more.
const MAX_HOPS: usize = 3;

/// A trip shorter than this is a stroll to the tram stop, not the longest walk of a month.
pub const MIN_LONGEST_M: f64 = 1000.0;

/// Arc types the taxi to the runway and a split leg of a few hundred metres as airplane too.
/// Those are not flights anyone would put in a feed; anything under this is left out.
pub const MIN_FLIGHT_M: f64 = 50_000.0;

/// The activity types a "longest" highlight is reported for, with the noun a title uses.
const LONGEST_TYPES: [(ActivityType, &str); 4] = [
    (ActivityType::Walking, "walk"),
    (ActivityType::Running, "run"),
    (ActivityType::Cycling, "ride"),
    (ActivityType::Hiking, "hike"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Country,
    Locality,
    Flight,
    Longest,
}

impl Kind {
    /// Also the order highlights of the same day are reported in.
    pub const ALL: [Kind; 4] = [Kind::Country, Kind::Locality, Kind::Flight, Kind::Longest];

    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Country => "country",
            Kind::Locality => "locality",
            Kind::Flight => "flight",
            Kind::Longest => "longest",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Kind::ALL.into_iter().find(|kind| kind.as_str() == name)
    }

    fn order(self) -> usize {
        Kind::ALL.iter().position(|&kind| kind == self).unwrap_or(0)
    }
}

/// A day summary, reduced to what a highlight is derived from.
#[derive(Debug, Clone)]
pub struct DayRow {
    pub date: String,
    pub country_codes: Vec<String>,
    pub localities: Vec<String>,
    pub confirmed: bool,
}

impl DayRow {
    /// The day's country codes as ISO writes them. Arc stores whatever the source had, so the
    /// same country arrives as "de" from a place and "DE" from a visit, and a day that saw only
    /// Germany can end up with two codes in it.
    fn codes(&self) -> Vec<String> {
        let mut codes: Vec<String> = Vec::new();
        for code in &self.country_codes {
            let code = code.trim().to_uppercase();
            if !code.is_empty() && !codes.contains(&code) {
                codes.push(code);
            }
        }
        codes
    }

    /// The country a locality on this day belongs to, when there is no doubt about it. There is
    /// a Paris in Texas, so a locality is only ever a first time within a country.
    fn only_country(&self) -> Option<String> {
        match self.codes().as_slice() {
            [code] => Some(code.clone()),
            _ => None,
        }
    }
}

/// A trip, reduced the same way.
#[derive(Debug, Clone)]
pub struct TripRow {
    pub item_id: String,
    pub date: String,
    pub activity_type: ActivityType,
    pub distance_m: Option<f64>,
    pub duration_seconds: i64,
    pub confirmed: bool,
}

/// A flight and the places either end of it, already resolved.
#[derive(Debug, Clone)]
pub struct FlightRow {
    pub trip: TripRow,
    pub from: Option<Endpoint>,
    pub to: Option<Endpoint>,
}

/// Where a flight left from or arrived at, as far as the neighbouring visit knows.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub locality: Option<String>,
    pub country_code: Option<String>,
    pub place_name: Option<String>,
}

impl Endpoint {
    /// What to call this end of the flight in a title. The locality is the useful half of an
    /// airport's name ("Flight from Dresden to Lisbon"); the place name is the fallback for a
    /// visit that never got one.
    fn label(&self) -> Option<&str> {
        self.locality
            .as_deref()
            .or(self.place_name.as_deref())
            .filter(|label| !label.is_empty())
    }

    fn is_empty(&self) -> bool {
        self.locality.is_none() && self.country_code.is_none() && self.place_name.is_none()
    }

    /// Country codes come out of Arc in whichever case the source had; highlights report the
    /// uppercase ISO 3166-1 writes, here as well as on a first time.
    fn normalized(mut self) -> Self {
        self.country_code = self
            .country_code
            .map(|code| code.trim().to_uppercase())
            .filter(|code| !code.is_empty());
        self
    }
}

/// One item on the way to a flight's endpoint, for [`nearest_visit`] to walk over.
#[derive(Debug, Clone)]
pub struct Neighbour {
    pub is_visit: bool,
    pub previous_item_id: Option<String>,
    pub next_item_id: Option<String>,
    pub endpoint: Endpoint,
}

#[derive(Debug, Clone, Copy)]
pub enum Direction {
    Previous,
    Next,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Highlight {
    pub kind: Kind,
    pub date: String,
    pub title: String,
    /// Whether the data behind this event has been reviewed in Arc: the day's flag for a first
    /// time, the item's for a flight or a longest trip.
    pub confirmed: bool,
    #[serde(flatten)]
    pub detail: Detail,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub enum Detail {
    Country {
        country_code: String,
        /// The English short name, null for a code ISO 3166-1 does not have.
        country: Option<String>,
    },
    Locality {
        locality: String,
        /// Null when the day touched more than one country and there is no telling which.
        country_code: Option<String>,
    },
    Flight {
        item_id: String,
        distance_m: Option<f64>,
        duration_seconds: i64,
        from: Option<Endpoint>,
        to: Option<Endpoint>,
    },
    Longest {
        item_id: String,
        activity_type: String,
        distance_m: f64,
        duration_seconds: i64,
    },
}

/// Everything the events of a range are derived from.
#[derive(Clone, Copy)]
pub struct Input<'a> {
    /// The inclusive range the events are reported for, as local days.
    pub from: &'a Day,
    pub to: &'a Day,
    /// Every day summary up to and including `to`, in date order. A first time is first over
    /// all of history, not just inside the range, so the earlier days have to be here too.
    pub days: &'a [DayRow],
    /// Flights that started inside the range.
    pub flights: &'a [FlightRow],
    /// Candidates for the longest trip per activity type, inside the range.
    pub trips: &'a [TripRow],
    pub kinds: &'a [Kind],
    /// Drop everything the user has not reviewed yet.
    pub confirmed_only: bool,
}

pub fn compute(input: &Input<'_>) -> Vec<Highlight> {
    let wants = |kind: Kind| input.kinds.contains(&kind);
    let mut highlights = Vec::new();
    if wants(Kind::Country) {
        highlights.extend(countries(input.days, input.from, input.to));
    }
    if wants(Kind::Locality) {
        highlights.extend(localities(input.days, input.from, input.to));
    }
    if wants(Kind::Flight) {
        highlights.extend(
            input
                .flights
                .iter()
                .filter(|row| row.trip.distance_m.unwrap_or(0.0) >= MIN_FLIGHT_M)
                .map(flight),
        );
    }
    if wants(Kind::Longest) {
        highlights.extend(longest(input.trips));
    }
    if input.confirmed_only {
        highlights.retain(|highlight| highlight.confirmed);
    }
    // Date first, then the kinds in their declared order, then the title, so the same request
    // always renders the same feed.
    highlights.sort_by(|a, b| {
        (&a.date, a.kind.order(), &a.title).cmp(&(&b.date, b.kind.order(), &b.title))
    });
    highlights
}

/// The first local day each country code was ever seen on, reported when that day is in range.
pub fn countries(days: &[DayRow], from: &Day, to: &Day) -> Vec<Highlight> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut highlights = Vec::new();
    for day in days.iter().filter(|day| day.date.as_str() <= to) {
        for code in day.codes() {
            if !seen.insert(code.clone()) || day.date.as_str() < from {
                continue;
            }
            let country = country_name(&code);
            highlights.push(Highlight {
                kind: Kind::Country,
                date: day.date.clone(),
                title: format!("First time in {}", country.as_deref().unwrap_or(&code)),
                confirmed: day.confirmed,
                detail: Detail::Country {
                    country_code: code,
                    country,
                },
            });
        }
    }
    highlights
}

/// The same, per locality within a country.
pub fn localities(days: &[DayRow], from: &Day, to: &Day) -> Vec<Highlight> {
    // Which countries each name has been seen in, `None` for a day that could not say.
    let mut seen: HashMap<&str, HashSet<Option<String>>> = HashMap::new();
    let mut highlights = Vec::new();
    for day in days.iter().filter(|day| day.date.as_str() <= to) {
        let country = day.only_country();
        for locality in &day.localities {
            let countries = seen.entry(locality.as_str()).or_default();
            // A day that touched two countries cannot say which one the name belongs to, so it
            // is a first time only if the name is new outright. The other way round, a name
            // once seen without a country is not first again the day its country is known.
            let first = match country {
                Some(_) => !countries.contains(&country) && !countries.contains(&None),
                None => countries.is_empty(),
            };
            countries.insert(country.clone());
            if !first || day.date.as_str() < from {
                continue;
            }
            highlights.push(Highlight {
                kind: Kind::Locality,
                date: day.date.clone(),
                title: format!("First time in {locality}"),
                confirmed: day.confirmed,
                detail: Detail::Locality {
                    locality: locality.clone(),
                    country_code: country.clone(),
                },
            });
        }
    }
    highlights
}

fn flight(row: &FlightRow) -> Highlight {
    let ends = row
        .from
        .as_ref()
        .and_then(Endpoint::label)
        .zip(row.to.as_ref().and_then(Endpoint::label));
    let title = match (ends, row.trip.distance_m) {
        (Some((from, to)), _) => format!("Flight from {from} to {to}"),
        // Without both ends the distance is the only thing that says anything about the flight.
        (None, Some(distance)) => format!("Flight of {}", format_distance(distance)),
        (None, None) => "Flight".to_string(),
    };
    Highlight {
        kind: Kind::Flight,
        date: row.trip.date.clone(),
        title,
        confirmed: row.trip.confirmed,
        detail: Detail::Flight {
            item_id: row.trip.item_id.clone(),
            distance_m: row.trip.distance_m,
            duration_seconds: row.trip.duration_seconds,
            from: row.from.clone(),
            to: row.to.clone(),
        },
    }
}

/// The longest trip per activity type. Types with nothing worth reporting are simply absent.
pub fn longest(trips: &[TripRow]) -> Vec<Highlight> {
    let mut highlights = Vec::new();
    for (activity, noun) in LONGEST_TYPES {
        let winner = trips
            .iter()
            .filter(|trip| trip.activity_type == activity)
            .filter_map(|trip| Some((trip, trip.distance_m?)))
            .filter(|&(_, distance)| distance >= MIN_LONGEST_M)
            // Ties go to the earlier trip: `max_by` keeps the last of equal elements, and the
            // rows arrive in time order.
            .max_by(|(_, a), (_, b)| a.total_cmp(b));
        let Some((trip, distance)) = winner else {
            continue;
        };
        highlights.push(Highlight {
            kind: Kind::Longest,
            date: trip.date.clone(),
            title: format!("Longest {noun}: {}", format_distance(distance)),
            confirmed: trip.confirmed,
            detail: Detail::Longest {
                item_id: trip.item_id.clone(),
                activity_type: activity.as_str().into_owned(),
                distance_m: distance,
                duration_seconds: trip.duration_seconds,
            },
        });
    }
    highlights
}

/// The place either side of a flight: the nearest visit within [`MAX_HOPS`] links, or nothing.
///
/// `fetch` looks an item up by id. It is a closure so that the walk itself, which is the part
/// with a rule in it, stays testable without a database.
pub fn nearest_visit<F>(
    start: Option<&str>,
    direction: Direction,
    mut fetch: F,
) -> Result<Option<Endpoint>>
where
    F: FnMut(&str) -> Result<Option<Neighbour>>,
{
    let mut id = start.map(str::to_string);
    for _ in 0..MAX_HOPS {
        let Some(current) = id else { break };
        let Some(neighbour) = fetch(&current)? else {
            break;
        };
        if neighbour.is_visit {
            // A visit that knows nothing about where it is answers nothing, rather than an
            // endpoint of three nulls.
            let endpoint = neighbour.endpoint.normalized();
            return Ok((!endpoint.is_empty()).then_some(endpoint));
        }
        id = match direction {
            Direction::Previous => neighbour.previous_item_id,
            Direction::Next => neighbour.next_item_id,
        };
    }
    Ok(None)
}

/// The English short name for an ISO 3166-1 alpha-2 code, as ISO itself lists it.
pub fn country_name(code: &str) -> Option<String> {
    rust_iso3166::from_alpha2(&code.to_uppercase()).map(|country| country.name.to_string())
}

/// Distance for a title: whole metres below a kilometre, otherwise kilometres with a decimal
/// only when it says something. "62 km", "3.4 km", "980 m".
fn format_distance(metres: f64) -> String {
    if metres < 1000.0 {
        return format!("{} m", metres.round());
    }
    let km = metres / 1000.0;
    if km >= 100.0 {
        return format!("{} km", km.round());
    }
    let rendered = format!("{km:.1}");
    format!("{} km", rendered.strip_suffix(".0").unwrap_or(&rendered))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, countries: &[&str], localities: &[&str], confirmed: bool) -> DayRow {
        DayRow {
            date: date.to_string(),
            country_codes: countries.iter().map(|code| code.to_string()).collect(),
            localities: localities.iter().map(|name| name.to_string()).collect(),
            confirmed,
        }
    }

    fn trip(id: &str, date: &str, activity: ActivityType, distance: f64) -> TripRow {
        TripRow {
            item_id: id.to_string(),
            date: date.to_string(),
            activity_type: activity,
            distance_m: Some(distance),
            duration_seconds: 600,
            confirmed: true,
        }
    }

    fn titles(highlights: &[Highlight]) -> Vec<&str> {
        highlights
            .iter()
            .map(|highlight| highlight.title.as_str())
            .collect()
    }

    /// "First" means first ever, not first in the range: a country seen before `from` is old
    /// news however often it comes back.
    #[test]
    fn a_country_is_reported_only_on_the_day_it_was_first_seen() {
        let days = [
            day("2024-01-05", &["de"], &["Dresden"], true),
            day("2024-06-01", &["de"], &["Dresden"], true),
            day("2024-06-02", &["pt"], &["Lisboa"], true),
            day("2024-06-09", &["pt"], &["Porto"], false),
        ];

        let found = countries(&days, "2024-06-01", "2024-06-30");

        assert_eq!(titles(&found), ["First time in Portugal"]);
        assert_eq!(found[0].date, "2024-06-02");
        assert_eq!(
            found[0].detail,
            Detail::Country {
                country_code: "PT".to_string(),
                country: Some("Portugal".to_string()),
            }
        );
        // Germany was first seen well before the range, so nothing is reported for it.
        assert!(
            countries(&days, "2024-06-01", "2024-06-30")
                .iter()
                .all(|highlight| highlight.date != "2024-01-05")
        );
        // A day after `to` has not happened yet as far as this request is concerned.
        assert!(countries(&days, "2024-06-01", "2024-06-05").len() == 1);
    }

    /// Arc stores a country code as its source had it, so the same country shows up as "pt" one
    /// day and "PT" the next, and sometimes both on one day.
    #[test]
    fn a_country_is_the_same_country_whatever_its_case() {
        let days = [
            day("2024-06-02", &["pt", "PT"], &["Porto"], true),
            day("2024-06-09", &["PT"], &["Porto"], true),
        ];

        let found = countries(&days, "2024-06-01", "2024-06-30");

        assert_eq!(titles(&found), ["First time in Portugal"]);
        // One country on the day, so the locality is not in doubt either.
        assert_eq!(
            localities(&days, "2024-06-01", "2024-06-30")[0].detail,
            Detail::Locality {
                locality: "Porto".to_string(),
                country_code: Some("PT".to_string()),
            }
        );
    }

    /// There is a Paris in Texas, so the same name in another country is a first time again.
    #[test]
    fn a_locality_is_first_per_country() {
        let days = [
            day("2024-02-01", &["fr"], &["Paris"], true),
            day("2024-03-01", &["fr"], &["Paris"], true),
            day("2024-04-01", &["us"], &["Paris"], false),
            // Two countries in one day: which one the locality is in is not knowable, and a
            // name that has already been somewhere is not a first time again.
            day("2024-05-01", &["de", "cz"], &["Paris", "Praha"], true),
            // Nor the other way round: Praha is known now, but it is not new any more.
            day("2024-05-02", &["cz"], &["Praha"], true),
        ];

        let found = localities(&days, "2024-01-01", "2024-12-31");

        assert_eq!(
            titles(&found),
            [
                "First time in Paris",
                "First time in Paris",
                "First time in Praha"
            ],
            "{found:?}"
        );
        assert_eq!(found[0].date, "2024-02-01");
        assert_eq!(
            found[0].detail,
            Detail::Locality {
                locality: "Paris".to_string(),
                country_code: Some("FR".to_string()),
            }
        );
        assert_eq!(found[1].date, "2024-04-01");
        assert!(!found[1].confirmed);
        assert_eq!(found[2].date, "2024-05-01");
        assert_eq!(
            found[2].detail,
            Detail::Locality {
                locality: "Praha".to_string(),
                country_code: None,
            }
        );
    }

    #[test]
    fn the_longest_trip_per_type_needs_a_kilometre() {
        let trips = [
            trip("a", "2024-06-01", ActivityType::Walking, 900.0),
            trip("b", "2024-06-02", ActivityType::Walking, 4200.0),
            trip("c", "2024-06-03", ActivityType::Walking, 3100.0),
            trip("d", "2024-06-04", ActivityType::Cycling, 62_000.0),
            trip("e", "2024-06-05", ActivityType::Running, 800.0),
        ];

        let found = longest(&trips);

        assert_eq!(
            titles(&found),
            ["Longest walk: 4.2 km", "Longest ride: 62 km"]
        );
        assert_eq!(found[0].date, "2024-06-02");
        assert_eq!(
            found[1].detail,
            Detail::Longest {
                item_id: "d".to_string(),
                activity_type: "cycling".to_string(),
                distance_m: 62_000.0,
                duration_seconds: 600,
            }
        );
        // Running never got off the ground, and hiking never happened at all.
        assert!(longest(&trips[4..]).is_empty());
    }

    /// A flight is bracketed by visits, but a walk through the terminal in between is normal.
    #[test]
    fn a_flight_finds_the_visit_past_a_non_visit() {
        let items = |id: &str| -> Result<Option<Neighbour>> {
            Ok(match id {
                "walk" => Some(Neighbour {
                    is_visit: false,
                    previous_item_id: None,
                    next_item_id: Some("gate".to_string()),
                    endpoint: Endpoint::default(),
                }),
                "gate" => Some(Neighbour {
                    is_visit: true,
                    previous_item_id: None,
                    next_item_id: None,
                    endpoint: Endpoint {
                        locality: Some("Porto".to_string()),
                        country_code: Some("pt".to_string()),
                        place_name: Some("Aeroporto do Porto".to_string()),
                    },
                }),
                "nowhere" => Some(Neighbour {
                    is_visit: true,
                    previous_item_id: None,
                    next_item_id: None,
                    endpoint: Endpoint::default(),
                }),
                _ => None,
            })
        };

        let found = nearest_visit(Some("walk"), Direction::Next, items)
            .unwrap()
            .unwrap();
        assert_eq!(found.locality.as_deref(), Some("Porto"));
        // Arc had the code in lowercase; ISO 3166-1 writes it in caps and so does the API.
        assert_eq!(found.country_code.as_deref(), Some("PT"));

        // Nothing to walk from, an id that is not there, and a visit with nothing to say.
        assert!(
            nearest_visit(None, Direction::Next, items)
                .unwrap()
                .is_none()
        );
        assert!(
            nearest_visit(Some("gone"), Direction::Next, items)
                .unwrap()
                .is_none()
        );
        assert!(
            nearest_visit(Some("nowhere"), Direction::Next, items)
                .unwrap()
                .is_none()
        );
    }

    /// A chain of nothing but trips runs out rather than walking the whole timeline.
    #[test]
    fn the_walk_to_an_endpoint_gives_up_after_a_few_links() {
        let mut fetched = 0;
        let endless = |_: &str| -> Result<Option<Neighbour>> {
            fetched += 1;
            Ok(Some(Neighbour {
                is_visit: false,
                previous_item_id: Some("more".to_string()),
                next_item_id: Some("more".to_string()),
                endpoint: Endpoint::default(),
            }))
        };

        assert!(
            nearest_visit(Some("trip"), Direction::Previous, endless)
                .unwrap()
                .is_none()
        );
        assert_eq!(fetched, MAX_HOPS);
    }

    /// The taxi to the runway is airplane-typed too; it is not a flight worth a line in a feed.
    #[test]
    fn short_airplane_legs_are_not_flights() {
        let short = FlightRow {
            trip: trip("taxi", "2024-06-02", ActivityType::Airplane, 800.0),
            from: None,
            to: None,
        };
        let long = FlightRow {
            trip: trip("f", "2024-06-02", ActivityType::Airplane, 1_600_000.0),
            from: None,
            to: None,
        };
        let flights = [short, long];
        let input = Input {
            from: "2024-06-01",
            to: "2024-06-30",
            days: &[],
            flights: &flights,
            trips: &[],
            kinds: &[Kind::Flight],
            confirmed_only: false,
        };
        let found = compute(&input);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].title, "Flight of 1600 km");
    }

    #[test]
    fn events_are_sorted_by_day_then_kind_then_title() {
        let days = [
            day("2024-06-02", &["pt"], &["Porto", "Lisboa"], true),
            day("2024-06-03", &["pt"], &["Porto"], true),
        ];
        let flight = FlightRow {
            trip: TripRow {
                confirmed: false,
                ..trip("f", "2024-06-02", ActivityType::Airplane, 1_600_000.0)
            },
            from: Some(Endpoint {
                locality: Some("Dresden".to_string()),
                country_code: Some("de".to_string()),
                place_name: None,
            }),
            to: Some(Endpoint {
                locality: Some("Lisboa".to_string()),
                country_code: Some("pt".to_string()),
                place_name: None,
            }),
        };
        let trips = [trip("w", "2024-06-03", ActivityType::Walking, 8000.0)];
        let input = Input {
            from: "2024-06-01",
            to: "2024-06-30",
            days: &days,
            flights: std::slice::from_ref(&flight),
            trips: &trips,
            kinds: &Kind::ALL,
            confirmed_only: false,
        };

        let found = compute(&input);

        assert_eq!(
            titles(&found),
            [
                "First time in Portugal",
                "First time in Lisboa",
                "First time in Porto",
                "Flight from Dresden to Lisboa",
                "Longest walk: 8 km",
            ]
        );

        // One kind at a time, and only what has been reviewed.
        let flights_only = compute(&Input {
            kinds: &[Kind::Flight],
            ..input
        });
        assert_eq!(flights_only.len(), 1);
        let reviewed = compute(&Input {
            confirmed_only: true,
            ..input
        });
        assert_eq!(reviewed.len(), 4, "{reviewed:?}");
        assert!(reviewed.iter().all(|highlight| highlight.confirmed));
    }

    #[test]
    fn a_flight_without_both_ends_falls_back_to_its_distance() {
        let row = FlightRow {
            trip: trip("f", "2024-06-02", ActivityType::Airplane, 1_600_000.0),
            from: Some(Endpoint {
                locality: Some("Dresden".to_string()),
                country_code: Some("de".to_string()),
                place_name: None,
            }),
            to: None,
        };

        assert_eq!(flight(&row).title, "Flight of 1600 km");
        assert_eq!(
            flight(&FlightRow {
                trip: TripRow {
                    distance_m: None,
                    ..row.trip.clone()
                },
                from: None,
                to: None,
            })
            .title,
            "Flight"
        );
    }

    #[test]
    fn distances_read_the_way_a_person_would_say_them() {
        assert_eq!(format_distance(980.4), "980 m");
        assert_eq!(format_distance(1000.0), "1 km");
        assert_eq!(format_distance(3421.0), "3.4 km");
        assert_eq!(format_distance(62_000.0), "62 km");
        assert_eq!(format_distance(1_612_345.0), "1612 km");
    }

    #[test]
    fn country_codes_map_to_english_short_names() {
        assert_eq!(country_name("de").as_deref(), Some("Germany"));
        assert_eq!(country_name("PT").as_deref(), Some("Portugal"));
        assert_eq!(country_name("zz"), None);
    }

    #[test]
    fn kinds_round_trip_through_their_names() {
        for kind in Kind::ALL {
            assert_eq!(Kind::parse(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::parse("longest-ever"), None);
    }
}
