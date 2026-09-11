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
September 2026, a first run over three years of data derived 13 841 items and
936 day summaries out of 2.58 M samples in 3.6 s of an 82 s pass, and a full
`arches derive` took 6.1 s. A run that finds no changed bucket recomputes
nothing.

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
| `GET /api/days/{date}/geojson?simplify=` | FeatureCollection: a LineString per trip from its fixes, a Point per visit. |
| `GET /api/days/{date}.gpx` | GPX 1.1, a `<wpt>` per visit and a `<trk>` per trip. |
| `GET /api/items/{id}` | One item, with its place if it is a visit. |
| `GET /api/items/{id}/samples?simplify=` | The item's fixes in time order. |
| `GET /api/places?q=&country=&limit=` | Places by name, locality or street address, most visited first. |
| `GET /api/places/{id}` | One place. |
| `GET /api/places/{id}/visits?from=&to=&limit=` | Visits there, newest first. |
| `GET /api/near?lat=&lon=&radius=` | Places within a radius in metres (default 250, max 5000), nearest first, each with `distanceM`. |
| `GET /api/at?ts=` | The item covering an instant, preferring the visit; `{ "item": null }` when nothing does, not a 404. |

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
| `ARCHES_INGEST_INTERVAL`  | `15m`                                                                                  | How often ingest polls Arc's buckets (`s`/`m`/`h` suffix). |
| `RUST_LOG`                | `info`                                                                                 | Log filter (tracing `EnvFilter` syntax). |
