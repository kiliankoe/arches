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

See [PLAN.md](./PLAN.md) for the full design: data model, API surface and
phased implementation plan.

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
