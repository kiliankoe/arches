# arches

Bridge between Arc Timeline Recorder's iCloud backup and other tools. See
PLAN.md for the design and phased plan; it is the source of truth for
decisions in progress.

## Rules

- Arc's iCloud folder (`ARCHES_ARC_DIR`) is **read-only**, without exception.
  Never open it for writing, never create or delete anything in it. Tests
  that touch Arc-shaped data must use a temp dir (`tempfile`), never the
  real folder.
- Frontend package manager is pnpm, never npm.
- Stack mirrors the sibling project pensieve: Rust 2024, axum, tokio,
  rusqlite (bundled, phase 2+), serde, tracing, anyhow; React 19, Vite,
  TypeScript, react-router, maplibre-gl, vitest, biome (lint and format, spaces).
- Comments explain why, not what.
- Do not run `git commit` or `git push` unless explicitly asked.

## Commands

Backend, from the repo root inside `nix develop` (or direnv):

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run -- serve
```

Frontend:

```
pnpm --dir web install
pnpm --dir web lint     # biome check
pnpm --dir web format   # biome format --write
pnpm --dir web test
pnpm --dir web build
pnpm --dir web dev
```

Nix:

```
nix build
nix flake check
```
