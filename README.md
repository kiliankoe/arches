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
