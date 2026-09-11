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
