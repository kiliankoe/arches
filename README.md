# arches

arches is a bridge between the data the Arc Timeline Recorder app backs up
into iCloud and other tools on the tailnet, first of all pensieve. It ingests
the backup, and optionally a directory of older GPX history, into SQLite,
serves it over a JSON/GeoJSON API and ships a small React UI for browsing
days, weeks, months, a heatmap and highlights.

Arc's iCloud folder and the GPX directory are **read-only** for arches,
without exception. Files are opened to read, nothing is created, renamed or
deleted in there, and no temp files land next to the data. Everything arches
writes goes to `ARCHES_DATA_DIR`. An accidental write could corrupt Arc's
backup chain or trigger a restore.

## Getting started

The project uses a Nix flake and direnv:

```
direnv allow            # or: nix develop
pnpm --dir web install
```

For development run the backend and the Vite dev server side by side. Vite
proxies `/api` to the backend; `ARCHES_API_URL` points it elsewhere.

```
cargo run -- serve      # API on 127.0.0.1:8471, ingest every 15 minutes
pnpm --dir web dev      # UI on http://localhost:5173
```

The first pass ingests the whole backup and takes a few minutes, almost all
of it waiting for iCloud to download the buckets. Later passes only read
files that changed. `data/` is gitignored, so `ARCHES_DATA_DIR=data` keeps a
development database inside the checkout. To try the app without the real
backup, ingest the test fixtures:

```
ARCHES_DATA_DIR=data ARCHES_ARC_DIR=tests/fixtures/backup ARCHES_GPX_DIR=tests/fixtures/gpx cargo run -- ingest
```

Subcommands: `serve`, `ingest` (one pass), `derive` (rebuild every derived
row from scratch, for when the rules change) and `status [--json]`.

Checks:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
pnpm --dir web lint
pnpm --dir web test
```

Deployment is a single binary with the UI embedded. `nix build` does both
steps; by hand they are:

```
pnpm --dir web build
cargo build --release
```

`cargo build` embeds whatever is in `web/dist` at compile time. The folder is
gitignored and `build.rs` creates it empty, so a checkout that never built the
UI still compiles and `/` says the UI is missing.

On voyager arches runs as a launchd user agent bound to the tailnet address,
defined in `hosts/voyager/arches.nix` of the nix config. Both sources sit in
locations macOS guards with TCC (iCloud Drive and `~/Documents`), which a
background agent can only read once the binary has Full Disk Access. That
grant and "Keep Downloaded" on the Arc folder are the two manual steps.

## Configuration

Everything is configured through environment variables:

| Variable | Default | Meaning |
| --- | --- | --- |
| `ARCHES_ARC_DIR` | `~/Library/Mobile Documents/iCloud~com~bigpaua~Arc-Timeline-Editor/Documents` | Arc's iCloud `Documents` dir. Read-only. |
| `ARCHES_GPX_DIR` | none | Directory of daily GPX files to ingest as history. Read-only. Unset means no GPX ingest. |
| `ARCHES_DATA_DIR` | `~/Library/Application Support/arches` | Where the database and the raw mirror go. |
| `ARCHES_BIND` | `127.0.0.1:8471` | Address the HTTP API binds to. No auth, so exposing it is a deployment choice. |
| `ARCHES_MAP_STYLE` | `https://tiles.openfreemap.org/styles/liberty` | MapLibre style URL for the frontend. |
| `ARCHES_MAP_STYLE_DARK` | `https://tiles.openfreemap.org/styles/dark` | Style the heatmap renders on. |
| `ARCHES_INGEST_INTERVAL` | `15m` | How often `serve` polls for changes (`s`, `m` or `h` suffix). |
| `RUST_LOG` | `info` | Log filter in tracing `EnvFilter` syntax. |

## How it works

- Arc backs up to `<ARCHES_ARC_DIR>/Backup/<device-uuid>/` in LocoKit2's
  bucketed export format, specified in
  [docs/export/FORMAT.md](https://github.com/sobri909/LocoKit2/blob/main/docs/export/FORMAT.md):
  places bucketed by UUID prefix, items by month, samples by ISO week, plain
  or gzipped. arches reads the device whose backup finished most recently.
  Files iCloud has evicted are skipped and logged.
- An ingest pass compares each bucket's mtime and size with what it saw last
  time, copies a changed file into `<ARCHES_DATA_DIR>/raw/` and parses the
  copy, so an evicted file is downloaded once and the mirror doubles as an
  off-iCloud backup. Records are upserted and an older `lastSaved` never
  overwrites a newer one. Nothing is deleted; Arc marks removals with
  `deleted`. A file that fails to parse is logged and left for the next pass,
  the rest still go in.
- `ARCHES_GPX_DIR` is an optional second source: one GPX file per UTC day, the
  shape Arc's own exports have. Tracks become trips, waypoints become visits
  and named places, all under the file's `creator` as their `source`. Ids are
  hashes of file and element, so a re-export upserts in place, and a visit
  that spans midnight appears in both files and merges into one item. A file
  dated on or after Arc's first day is skipped: Arc has ids, accuracies and
  confirmed places, the import has none of that. GPX carries no addresses or
  country codes either, so the history produces no first-time highlights.
- A day is a local day, never the UTC day. Arc records `secondsFromGMT` on
  samples and places; GPX times are UTC and get their offset from the
  coordinate (tzf-rs for the zone, jiff for the offset). Items get separate
  start and end offsets, since a flight lands in another one, and an item
  running over local midnight is clipped into both days.
- Ingest derives what it touched: a summary per day (distance and duration
  per activity type, visits, places, countries, bbox) and heatmap cells on a
  zoom 22 web-mercator grid with coarser rollups. Distance is summed between
  consecutive fixes of the same trip, never over a gap longer than ten minutes
  and never from a fix worse than 200 m of accuracy. A day only gets a row if
  it recorded something, so recording gaps stay gaps. `arches derive` rebuilds
  all of it when the rules change.
- Arc marks a visit confirmed once its place is picked and a trip once its
  activity type is; until then both are guesses that may change. Items carry
  `confirmed` and `uncertain`, a day is `confirmed` only when every item on it
  is, and highlights carry the flag along. Filtering on it is the consumer's
  job; `?confirmed=true` does it server-side.

## API

Everything lives under `/api` and speaks JSON, except the GeoJSON and GPX
renderings. Keys are camelCase, dates are local `YYYY-MM-DD`, timestamps are
RFC 3339, activity types are their enum names. Errors are `{ "error": "..." }`
with 400, 404 or 500. CORS allows any origin: the API is tailnet-only with no
auth and pensieve's frontend fetches from it directly.

| Endpoint | |
| --- | --- |
| `GET /api/status` | Last ingest run, Arc's `lastBackupDate`, row counts per source. |
| `GET /api/config` | Map style URLs for the frontend. |
| `POST /api/ingest` | Run one pass now. |
| `GET /api/days?from=&to=` | Day summaries, default the last 30 days, at most 400. |
| `GET /api/days/{date}` | The day's summary and its items in start order. |
| `GET /api/days/{date}/geojson?simplify=` | A LineString per trip, a Point per visit. |
| `GET /api/days/geojson?from=&to=&simplify=` | The same over a range of at most 62 days. |
| `GET /api/days/{date}.gpx` | GPX 1.1 rendering. |
| `GET /api/items/{id}` | One item, with its place if it is a visit. |
| `GET /api/items/{id}/samples?simplify=` | The item's fixes in time order. |
| `GET /api/places?q=&country=&limit=` | Places by name, locality or address, most visited first. |
| `GET /api/places/{id}` | One place. |
| `GET /api/places/{id}/visits?from=&to=&limit=` | Visits there, newest first. |
| `GET /api/near?lat=&lon=&radius=` | Places within a radius in metres, nearest first. |
| `GET /api/at?ts=` | The item covering an instant, `{ "item": null }` when none. |
| `GET /api/heatmap?bbox=&zoom=&from=&to=&weight=` | Cell-centre Points with a `weight`, binned for the viewport, plus `meta`. `weight=days` (default) counts distinct days per cell so a night at home does not outweigh every route; `samples` sums fixes. |
| `GET /api/highlights?from=&to=&kinds=&confirmed=` | First day in a country or a locality, flights of at least 50 km, the longest walk, run, ride and hike. "First" means first ever, not first in the range. At most 400 days. |

`simplify=<metres>` runs Ramer-Douglas-Peucker with that tolerance. Traces
drop fixes worse than 200 m of accuracy.

While `arches serve` runs, ingest polls every `ARCHES_INGEST_INTERVAL`,
starting immediately. The UI is served at `/`, with unknown paths falling
back to `index.html` so client-side routes deep-link.

## Web UI

A full-bleed MapLibre map on the OpenFreeMap style with a rail of chrome over
it; below 720 px the rail becomes a bottom sheet. Every route deep-links.

| Route | |
| --- | --- |
| `/` | Redirects to today. |
| `/day/{date}` | The day's track and timeline. Arrow keys step to the previous and next day with data. |
| `/week/{yyyy-Www}` | Seven 24 hour strips, items drawn as blocks. |
| `/month/{yyyy-mm}` | A calendar, each day a distance bar by activity type. |
| `/place/{id}` | One place and its visits. |
| `/heat?from=&to=&weight=` | The heatmap over a range, on the dark style. |
| `/highlights?from=&to=` | The notable events of a range, grouped by month. |

Colour means the activity type and nothing else, defined once in
`web/src/lib/activity.ts`. Unconfirmed items are drawn dashed. Times render
in the offset the day was recorded in, never the browser's timezone.
