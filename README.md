# arches

arches is a bridge between the data the Arc Timeline Recorder app backs up
into iCloud and other tools on the tailnet, first of all pensieve. It
ingests the Arc backup into SQLite, serves it over a JSON/GeoJSON API and
ships a small React UI for browsing days, weeks, months and heatmaps.

Arc's iCloud folder (`Documents/Backup/<device-uuid>/`, the LocoKit2
bucketed export format) is the source of truth and is **read-only** for
arches, without exception: files are opened read-only, nothing is ever
created, renamed, touched or deleted in there, and no temp files land next
to the data. Everything arches writes goes to `ARCHES_DATA_DIR`, including a
mirror of the raw bucket files. Tests use a temp dir, never the real folder.
An accidental write could corrupt Arc's backup chain or trigger a restore.

## Arc data format

Arc backs up to `<ARCHES_ARC_DIR>/Backup/<device-uuid>/`, one directory per
device. When there is more than one, arches uses the device whose backup
session finished most recently and logs the rest.

```
Backup/<device-uuid>/
  metadata.json          schema version, session dates, record counts
  places/{0-9A-F}.json   places bucketed by the first character of their UUID
  items/YYYY-MM.json     timeline items by month
  samples/YYYY-Www.json.gz  location samples by ISO week, usually gzipped
  notes/                 app extension data, unused here
```

Places and samples files hold plain arrays; item files hold wrapper objects
`{ "base": ..., "visit": ... }` or `{ "base": ..., "trip": ... }`. Both
`.json` and `.json.gz` are accepted everywhere. Null fields are omitted
rather than emitted, and Arc adds fields between schema versions, so
`src/arc/types.rs` treats almost everything as optional and never rejects
unknown fields. Evicted files show up as `.<name>.icloud` placeholders;
those are skipped and logged.

The format is LocoKit2's bucketed export, specified in
[docs/export/FORMAT.md](https://github.com/sobri909/LocoKit2/blob/main/docs/export/FORMAT.md).
Activity, moving and recording states are the integer enums from that repo;
arches keeps their raw values and renders them by name.

## Ingest

`arches ingest` runs one pass over the backup and `arches status` (or
`arches status --json`) reports what it left behind:

```
arches ingest
arches status
```

A pass discovers the newest device backup, lists the places, items and sample
buckets and compares each file's mtime and size against the `ingest_files`
table. Unchanged files are skipped. A changed file is first copied into
`<ARCHES_DATA_DIR>/raw/<device-uuid>/<bucket path>` by writing a temp file
next to the destination and renaming it, so the mirror is never partial, then
parsed from the mirrored copy. Parsing the copy rather than the original means
an iCloud-evicted file is downloaded exactly once per change, and the mirror
doubles as an off-iCloud backup of the raw buckets. A copy that takes longer
than three seconds is logged with its elapsed time, since a first run is
mostly spent waiting for iCloud.

Records are upserted in one transaction per file, guarded on `lastSaved`, so
an older rendering of a record never overwrites a newer one and re-ingesting
a file is free. Nothing is ever deleted; Arc marks removals with `deleted`.
The `ingest_files` row is written in the same transaction as the records, so
a crash re-ingests the file instead of losing it. A file that fails to parse
is logged, recorded in the run's `error` column and left un-ingested; the
other files still go in, and `arches ingest` exits non-zero.

Timestamps are stored as unix milliseconds and booleans as 0/1, so every
column is a plain integer.

Observed on cassini in September 2026, over three years of recording (183
bucket files: 16 place buckets, 32 item months, 135 sample weeks):

| | first run | second run |
| --- | --- | --- |
| wall time | 353.5 s | 0.02 s |
| files ingested | 183 | 0 |
| records upserted | 1134 places, 13 831 items, 2 576 927 samples | none |

That first run was almost entirely iCloud: 17 s of user and 24 s of system
time against six minutes of wall clock, with 43 buckets logged as slow copies
while iCloud fetched them. No file failed to parse. The result is a 746 MiB
database and a 328 MiB raw mirror, and every later run that finds nothing
changed costs milliseconds.

## Derived data

Everything the API and UI need per day is precomputed on ingest into
`day_summaries`, one row per **local** day. A day is never the UTC day: Arc
records `secondsFromGMT` on every sample and every place, and that offset is
what decides which day a record belongs to. Samples get a `local_date` on
insert, so it can never drift from the row.

Items carry no timezone of their own, so each one gets a `start_offset_seconds`
and an `end_offset_seconds` derived from its first and last sample that has an
offset. The two are resolved separately because a flight legitimately takes off
in one offset and lands in another. When an item has no sample with an offset,
the fallbacks are, in order: the visit's place, the previous item's end offset,
the next item's start offset, and finally UTC with a warning naming the item.
`local_start_date` and `local_end_date` follow from the offsets.

An item that runs over local midnight belongs to both days. Its duration is
clipped into each: the day window runs from local midnight at the item's start
offset to the next local midnight at its end offset, so the night the clocks
change is still measured correctly and nothing produces a negative or a
25-hour item. Visits count under `stationary` in `duration_by_type`, trips
under their resolved activity type.

Distance is measured from the samples rather than prorated from the item, and
three rules keep it honest:

- Only between consecutive samples of the same trip item, never across an item
  boundary and never inside a visit.
- Never across a gap of more than ten minutes. Arc sleeps when nothing is
  happening, and bridging a gap would draw a straight line through the night.
- Never from a fix with a horizontal accuracy worse than 200 m. Those are
  noise; they are dropped from the chain but still counted as samples and still
  stretch the day's bounding box.

Deleted and disabled items are excluded everywhere: Arc keeps removed items in
the export as tombstones, and a disabled item is one the user switched off. A
day gets a row only if it recorded something: at least one sample, or at least
one item that starts or ends on it. An item merely spanning a day is not
enough, because Arc renders a months-long recording gap as a single stretched
item. The April to July 2025 gap is one four-month "tram" trip with two
samples on it, and those 122 days have nothing in them to summarize. Nothing
may assume the summarized range is continuous.

Ingest derives only what a run touched: every item in a changed `items/` month
bucket, every item a changed `samples/` week bucket points at, and the local
days those cover padded by a day on each side. When the derivation rules
themselves change, `arches derive` rebuilds every item and every day from
scratch.

Derivation is cheap next to the ingest it rides along with. On cassini in
September 2026, a first run over three years of data derived 13 853 items and
936 day summaries out of 2.58 M samples in 10.6 s of a 96 s pass, of which
4.9 s went on the heatmap cells below (about 30 s for a full `arches derive`
once trips are rasterized into cells). A run that finds no changed bucket
recomputes nothing.

## Heatmap

Every fix is also counted into `heatmap_cells`, one row per local day per cell
of the web-mercator grid at zoom 22: the pixel grid of zoom 14 tiles, about 6 m
across at 51 N. The selection is the day summary's, minus the fixes worse than
200 m of horizontal accuracy, which would smear a whole neighbourhood into one
6 m cell. Trips are rasterized as well as sampled: the cells on the straight
line between two consecutive fixes of the same trip are counted too, under the
distance rules (never across an item boundary, never inside a visit, never over
a gap longer than ten minutes, and never for a leg longer than 2000 cells,
which only a flight produces). Fixes alone land 20 to 30 m apart at cycling
speed, so on a 6 m grid a route was a string of beads with neighbouring cells
carrying unrelated day counts, and no kernel radius could smooth that. Cells
are filled and cleared in the same pass that writes the day
summary, so a day that loses its row loses its cells with it and `arches
derive` rebuilds the table along with everything else. An existing database
migrating to schema 4 comes out with the tables empty; one `arches derive`
fills them.

Coarser grids are a right shift of the indices, `x >> s, y >> s`, so one fine
table serves every map zoom. `/api/heatmap` picks `s` from the requested zoom
so that a cell lands on about four screen pixels, which MapLibre's heatmap
layer then blurs; a full 1600 x 900 viewport comes out at up to about 30 000
points at street zoom. Output is capped at 40 000 points, and going over means
coarsening by another level and asking again rather than truncating: half a
heatmap is a lie about where someone was.

`weight=days`, the default, counts the distinct days a cell was recorded on.
`weight=samples` sums the fixes. Days is the default because a night at home is
thousands of samples in one cell and would wash out every route ever taken.
`meta` carries `cellMetres`, `maxWeight`, `points`, `days` (how many days in
the range put anything into the view) and `bbox` (the matched cells' extent,
for framing the range on first load).

Grouping the fine table turned out to cost about a third of a second for the
all-time views: millions of rows once trips are rasterized, and a bounding box
only seeks on `x`. So `heatmap_rollup` keeps the same per-day counts
pre-shifted to every level from 1 to 14, and a query reads the level it
actually wants. Both tables are keyed date first, so recomputing one day
deletes a contiguous range rather than scanning every other day. The spatial
indexes carry `samples` so a query is answered from the index alone; without
that, every row cost a lookup back into the table and a street-level viewport
took ten times as long. Rows are aggregated in Rust in one pass rather than
with `GROUP BY` and `count(DISTINCT date)`, which cost SQLite a temp b-tree
per group and a second scan for the day total.

Warm timings on cassini in September 2026, over 936 days and 2.58 M samples,
for a 1600 x 900 viewport over Dresden, all time unless noted:

| Zoom | | |
| --- | --- | --- |
| 3 | 2 ms | 189 points |
| 6 | 4 ms | 945 points |
| 12 | 61 ms | 10 023 points |
| 13 | 216 ms | 17 914 points |
| 15 | 150 ms | 27 473 points |
| 15, last 30 days | 51 ms | 5 887 points |
| 17 | 69 ms | 7 118 points |

About a third of the street-level time is gzip on the 3.4 MB of GeoJSON, which
goes over the wire as 165 KB.

## Confirmation

Arc marks whether the user has reviewed an item: a visit is confirmed once its
place is picked, a trip once it has a confirmed activity type, which Arc leaves
unset until someone says so. Until then the place and the activity type are the
app's guesses and may change under a consumer's feet. Both flags travel with
every item as `confirmed` and `uncertain`, and a day summary carries
`unconfirmedItems` plus a `confirmed` that is true only when every item counted
on that day is confirmed. A day with no items at all is confirmed: there is
nothing left to review. The most recent days are usually unconfirmed, since the
backup on disk tends to predate the review.

## Highlights

`/api/highlights` turns what is already recorded into the handful of events
worth a line in a feed or a journal, derived per request rather than stored:
kilko.de is meant to pick them up later. Four kinds, each an object with
`kind`, `date`, `title`, `confirmed` and its own fields:

| Kind | |
| --- | --- |
| `country` | The first local day a country code appears in a day summary, with `countryCode` and `country`, the ISO 3166-1 English short name. |
| `locality` | The same for a locality, with `locality` and, when the day touched exactly one country, `countryCode`. |
| `flight` | A trip whose resolved activity type is `airplane`, with `itemId`, `distanceM`, `durationSeconds` and `from` and `to` as `{ locality, countryCode, placeName }`. |
| `longest` | The longest walk, run, ride and hike of the range, each at least a kilometre, with `itemId`, `activityType`, `distanceM` and `durationSeconds`. |

Country codes are reported in the uppercase ISO 3166-1 writes, on a first
time and on a flight's endpoints alike: Arc stores whichever case the source
had, so the same country arrives as `de` from a place and `DE` from a visit,
and comparing them raw reported Germany twice.

"First" means first over all of history, not first in the range: a country or
a town first seen before `from` is not reported however often it comes back.
That is why the endpoint reads every day summary up to `to` rather than only
the ones in the range; at about a thousand rows a year that is cheaper than
keeping a table of firsts in step with a backup Arc rewrites retroactively.
Localities repeat across countries, so a first time is keyed on the pair: the
Paris in Texas is its own event. A day that touched two countries cannot say
which one a name belongs to, so it only counts if the name is new outright.

A flight has to be at least 50 km: Arc types the taxi to the runway and split
legs of a few hundred metres as airplane too, and those are not flights anyone
would put in a feed. A flight's endpoints are the nearest visit before and after it, found by
walking up to three `previousItemId` / `nextItemId` links past non-visits,
because a walk through the terminal between the gate and the flight is normal.
Anything further away is not the airport any more and the endpoint is null,
which makes the title fall back from "Flight from Dresden to Lisboa" to the
distance.

Every highlight carries `confirmed`: the day's flag for a first time, the
item's for a flight or a longest trip (see *Confirmation* above). **Filtering
on it is the consumer's job.** A public feed should drop what has not been
reviewed yet, since the place or the activity type behind it can still change;
`?confirmed=true` does that server-side for a consumer that would rather not
think about it. `kinds=` takes any comma-separated subset of the four, in any
order; `from` and `to` are both required and at most 400 days apart. Events
come back sorted by date, then by kind in the order above, then by title.

## API

Everything lives under `/api` and speaks JSON, except the GeoJSON and GPX
renderings. Keys are camelCase, dates in paths and query strings are local
`YYYY-MM-DD`, timestamps in responses are RFC 3339 strings, activity types are
their enum names and local offsets appear as `utcOffsetSeconds`. Errors are
`{ "error": "..." }` with 400 for a bad parameter, 404 for an id or date that
is not there and 500 otherwise, the cause logged rather than returned. CORS
allows any origin for GET and POST without credentials: the API is tailnet-only
with no auth and pensieve's browser frontend fetches from it directly.

| Endpoint | |
| --- | --- |
| `GET /api/status` | Version, last ingest run, Arc's `lastBackupDate`, newest bucket mtime, row counts, summarized date range and whether a pass is running. |
| `GET /api/config` | The MapLibre style URL the frontend renders with. |
| `POST /api/ingest` | Runs one pass now and returns its summary; queues behind the periodic one. |
| `GET /api/days?from=&to=` | Day summaries in an inclusive range, default the last 30 days, at most 400. Days with no row are absent. |
| `GET /api/days/{date}` | The day's summary plus its items in start order, each with the seconds it spent inside that day. |
| `GET /api/days/{date}/geojson?simplify=` | FeatureCollection: a LineString per trip from its fixes, a Point per visit, every feature carrying its local `date`. |
| `GET /api/days/geojson?from=&to=&simplify=` | The same rendering over an inclusive range, every day in one FeatureCollection. `from` and `to` are required and at most 62 days apart; days with no row contribute nothing, so an empty range is an empty collection rather than a 404. |
| `GET /api/days/{date}.gpx` | GPX 1.1, a `<wpt>` per visit and a `<trk>` per trip. |
| `GET /api/items/{id}` | One item, with its place if it is a visit. |
| `GET /api/items/{id}/samples?simplify=` | The item's fixes in time order. |
| `GET /api/places?q=&country=&limit=` | Places by name, locality or street address, most visited first. |
| `GET /api/places/{id}` | One place. |
| `GET /api/places/{id}/visits?from=&to=&limit=` | Visits there, newest first. |
| `GET /api/near?lat=&lon=&radius=` | Places within a radius in metres (default 250, max 5000), nearest first, each with `distanceM`. |
| `GET /api/at?ts=` | The item covering an instant, preferring the visit; `{ "item": null }` when nothing does, not a 404. |
| `GET /api/heatmap?bbox=&zoom=&from=&to=&weight=` | Everywhere you have been in a range, binned for the viewport: a FeatureCollection of cell-centre Points with a `weight`, plus a `meta` member. `bbox` and `zoom` are required. See *Heatmap* above. |
| `GET /api/highlights?from=&to=&kinds=&confirmed=` | The notable events of a range: first time in a country or a town, flights, the longest trip per activity type. `from` and `to` are required and at most 400 days apart. See *Highlights* above. |

`simplify=<metres>` runs Ramer-Douglas-Peucker over the coordinate chain with
that tolerance, measured on a local equirectangular projection so the number is
metres rather than degrees. Without it the full trace is returned. A trace drops
fixes worse than 200 m of horizontal accuracy, the same ones the derivation
refuses to measure distance from.

The React UI is embedded in the binary and served at `/`, with any path that is
not a file and not under `/api` falling back to `index.html` so client-side
routes deep-link. `web/dist` is gitignored, so a checkout that never ran
`pnpm --dir web build` still builds: `build.rs` creates the directory and `/`
answers with a plain note saying the UI is not in this binary.

While `arches serve` is running, ingest polls Arc's buckets every
`ARCHES_INGEST_INTERVAL`, starting immediately. It runs on a connection of its
own; requests read through separate connections, which WAL lets them do while a
pass is writing.

## Web UI

A React app in `web/`, built with Vite and embedded in the binary. The map is
the subject: a full-bleed MapLibre canvas on the OpenFreeMap style from
`/api/config`, with a rail of chrome floating over it. Below 720 px the rail
becomes a bottom sheet.

| Route | |
| --- | --- |
| `/` | Redirects to today's local day. |
| `/day/{date}` | The day's track on the map and its timeline in the rail: time range, place or activity, distance or duration, totals and an unconfirmed count. |
| `/month/{yyyy-mm}` | A calendar of the month, each day a distance bar segmented by activity type. |
| `/week/{yyyy-Www}` | Seven 24 hour strips, one per ISO-week day, items drawn as blocks by clipped time. |
| `/place/{id}` | One place, its address and counts, and its visits newest first. |
| `/heat?from=&to=&weight=` | The heatmap over a range of days, with presets, two date inputs and a days/samples toggle. Both travel in the URL. |
| `/highlights?from=&to=` | The notable events of a range, grouped by month, each linking to its day; an unconfirmed one is marked with a dashed rule. |

The week and month views frame their range and draw it too: both ask
`/api/days/geojson` for the whole range as one collection, simplified to 15 m
and 25 m respectively, and the month drops the visit points because a few
hundred of them at that zoom read as a carpet rather than as places.

Every route deep-links; unknown paths fall back to the shell, so the browser's
address bar is a usable input. In the day view the left and right arrow keys
step to the previous and next day *that has data*: the navigation asks
`/api/days` for a window around the date rather than walking into the middle of
a four-month recording gap.

Colour means exactly one thing, the activity type, and it is defined once in
`web/src/lib/activity.ts` and mirrored into CSS custom properties at startup so
the map lines, the timeline rules, the calendar bars and the week blocks cannot
disagree:

| | | | | | | | | |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| walking | running | cycling | car | bus | train | tram | airplane | other |
| `#3c7a1e` | `#a92e72` | `#0e7b72` | `#be3a2e` | `#b0690c` | `#2b4ca8` | `#7040a6` | `#1b6c93` | `#7a7420` |

The heatmap is the one thing that does not take a colour from that table. It
renders on the dark basemap from `ARCHES_MAP_STYLE_DARK` with a magma ramp,
transparent through dark purple, magenta, red and orange to yellow, so heat
climbs in luminance as well as hue against the dark ground. The kernel radius
follows the cell size each response reports, 3.5 times the cells' spacing on
screen, and doubles per zoom level until the next fetch so the cells never
drift apart from their kernels. MapLibre's kernel is a Gaussian with sigma at a
third of the radius, and its shader also shrinks a faint point's quad by its
weight, so anything much tighter than that, or a weight near zero, shows the
grid through the blur; weights therefore have a floor of 0.15. The weight
goes through a logarithm before it is normalised against the response's own
maximum. Counting days over three years, the cell holding a desk is 900 and a
street walked once is 1; linearly that street is a thousandth of full heat and
invisible, and a square root only gets it to a thirtieth. The logarithm puts it
at a tenth, which is the difference between a map of routes and a map of one
bright dot.

Visits are ink (`#1b2230`), not a hue. Confirmation state never takes a colour
of its own: an item Arc has not had confirmed is drawn dashed, on the map, in
the rail and in the calendar, so the palette keeps meaning activity.

Times are rendered in the offset the day was recorded in, from the API's
`utcOffsetSeconds` and the item's own start and end offsets, never in the
browser's timezone. A day in Bangkok reads in Bangkok time from anywhere.

maplibre-gl is loaded lazily, so the calendar and week grids render before the
map library arrives, and a map that cannot start (no WebGL) leaves the lists
intact rather than taking the page down. MapLibre parses vector tiles in a web
worker that it locates at runtime, which Vite cannot see; `MapView.tsx` imports
the worker with `?worker&url` and hands MapLibre that URL. Without it the
production build serves the SPA shell in place of the worker and the map shows
only the low-zoom raster layer.

### Building it into the binary

`cargo build` embeds whatever is in `web/dist` at compile time via `rust-embed`,
so the UI is built first:

```
pnpm --dir web install
pnpm --dir web build
cargo build --release
```

`web/dist` is gitignored; `build.rs` creates it empty so a fresh checkout still
compiles, and `/` then answers with a note saying the UI is not in this binary.
During development run `pnpm --dir web dev` instead and let Vite proxy `/api` to
`arches serve`.

## Development

This project uses a Nix flake and direnv:

```
direnv allow      # or: nix develop
cargo run -- serve
pnpm --dir web dev
```

The backend listens on `ARCHES_BIND` (default `127.0.0.1:8471`); the Vite
dev server proxies `/api` to it.

Backend checks:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Frontend checks (pnpm, not npm):

```
pnpm --dir web lint
pnpm --dir web format
pnpm --dir web test
pnpm --dir web build
```

## Configuration

All configuration is via environment variables:

| Variable                 | Default                                                                              | Meaning                                    |
| ------------------------ | ------------------------------------------------------------------------------------- | ------------------------------------------- |
| `ARCHES_ARC_DIR`          | `~/Library/Mobile Documents/iCloud~com~bigpaua~Arc-Timeline-Editor/Documents`          | Arc's iCloud `Documents` dir. Read-only.   |
| `ARCHES_DATA_DIR`         | `~/Library/Application Support/arches`                                                | Where arches writes its database and raw mirror. |
| `ARCHES_BIND`             | `127.0.0.1:8471`                                                                       | Address the HTTP API binds to.             |
| `ARCHES_MAP_STYLE`        | `https://tiles.openfreemap.org/styles/liberty`                                        | MapLibre style URL served to the frontend. |
| `ARCHES_MAP_STYLE_DARK`   | `https://tiles.openfreemap.org/styles/dark`                                           | Dark style the heatmap view renders on. |
| `ARCHES_INGEST_INTERVAL`  | `15m`                                                                                  | How often ingest polls Arc's buckets (`s`/`m`/`h` suffix). |
| `RUST_LOG`                | `info`                                                                                 | Log filter (tracing `EnvFilter` syntax). |
