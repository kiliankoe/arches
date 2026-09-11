//! Reading Arc's iCloud backup: the LocoKit2 bucketed export format.
//!
//! Format reference: `docs/export/FORMAT.md` in github.com/sobri909/LocoKit2. This module only
//! parses; the SQLite side lives in phase 2.

pub mod backup;
pub mod enums;
pub mod types;
