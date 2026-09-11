//! Copying bucket files out of iCloud into `<data_dir>/raw/<device_id>/`.
//!
//! The mirror is not just a backup. Ingest parses the mirrored copy rather than the iCloud file
//! so an evicted ("dataless") file is downloaded exactly once per change, and so a long parse
//! never holds a file handle open in Arc's folder.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Reading an evicted file blocks until iCloud has fetched it. Anything slower than this is
/// worth a line in the log, because it explains an otherwise mysteriously long run.
const SLOW_COPY: Duration = Duration::from_secs(3);

pub fn mirror_path(data_dir: &Path, device_id: &str, relative: &str) -> PathBuf {
    data_dir.join("raw").join(device_id).join(relative)
}

/// Copies `source` to its place in the mirror, atomically: the temp file lives next to the
/// destination inside the data dir (never in Arc's folder) and is renamed into place, so a
/// crash can leave a stray temp file but never a truncated mirror copy.
pub fn copy_into_mirror(source: &Path, destination: &Path) -> Result<u64> {
    let parent = destination
        .parent()
        .context("mirror destination has no parent")?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("creating mirror dir {}", parent.display()))?;

    let span = tracing::info_span!("mirror_copy", source = %source.display());
    let _entered = span.enter();
    let started = Instant::now();

    let file_name = destination
        .file_name()
        .context("mirror destination has no file name")?
        .to_string_lossy()
        .into_owned();
    let temp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));

    let bytes = {
        let mut reader =
            File::open(source).with_context(|| format!("opening {}", source.display()))?;
        let mut writer =
            File::create(&temp).with_context(|| format!("creating {}", temp.display()))?;
        let bytes = io::copy(&mut reader, &mut writer)
            .with_context(|| format!("copying {}", source.display()))?;
        writer.sync_all()?;
        bytes
    };
    std::fs::rename(&temp, destination)
        .with_context(|| format!("renaming into {}", destination.display()))?;

    let elapsed = started.elapsed();
    if elapsed >= SLOW_COPY {
        tracing::info!(
            ?elapsed,
            bytes,
            "slow bucket copy, likely an iCloud-evicted file"
        );
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_bytes_and_leaves_no_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("2025-W24.json");
        std::fs::write(&source, b"[1,2,3]").unwrap();
        let destination = mirror_path(temp.path(), "device", "samples/2025-W24.json");

        let bytes = copy_into_mirror(&source, &destination).unwrap();

        assert_eq!(bytes, 7);
        assert_eq!(std::fs::read(&destination).unwrap(), b"[1,2,3]");
        let strays: Vec<_> = std::fs::read_dir(destination.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .filter(|name| name.to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(strays.is_empty(), "{strays:?}");
    }

    #[test]
    fn overwrites_an_existing_copy() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("A.json");
        let destination = mirror_path(temp.path(), "device", "places/A.json");
        std::fs::write(&source, b"old").unwrap();
        copy_into_mirror(&source, &destination).unwrap();
        std::fs::write(&source, b"newer").unwrap();

        copy_into_mirror(&source, &destination).unwrap();

        assert_eq!(std::fs::read(&destination).unwrap(), b"newer");
    }
}
