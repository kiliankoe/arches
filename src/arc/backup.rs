//! Finding and reading the bucket files of an Arc backup.
//!
//! Everything in here opens files read-only and never writes to the Arc dir: see README.md.

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::{Context, Result, anyhow, bail};
use flate2::bufread::MultiGzDecoder;
use jiff::Timestamp;
use serde::de::DeserializeOwned;

use super::types::{LocomotionSample, Metadata, Place, TimelineItem};

/// Which kind of record a bucket file holds. The name also doubles as the sub-directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BucketKind {
    Places,
    Items,
    Samples,
}

impl BucketKind {
    pub fn dir_name(self) -> &'static str {
        match self {
            BucketKind::Places => "places",
            BucketKind::Items => "items",
            BucketKind::Samples => "samples",
        }
    }
}

/// A bucket file on disk. `mtime` and `len` are what phase 2 uses to skip unchanged buckets.
#[derive(Debug, Clone)]
pub struct BucketFile {
    pub path: PathBuf,
    pub kind: BucketKind,
    pub mtime: SystemTime,
    pub len: u64,
}

/// One device's backup directory, `<arc_dir>/Backup/<device-uuid>/`.
#[derive(Debug, Clone)]
pub struct Backup {
    pub dir: PathBuf,
    pub device_id: String,
}

impl Backup {
    /// Picks the device directory whose backup session finished most recently. Arc keeps stale
    /// dirs around after a device is replaced, and ingesting one of those would silently serve
    /// months-old data.
    pub fn discover(arc_dir: &Path) -> Result<Backup> {
        let root = arc_dir.join("Backup");
        let entries = std::fs::read_dir(&root)
            .with_context(|| format!("reading Arc backup dir {}", root.display()))?;

        let mut candidates: Vec<(Option<Timestamp>, Backup)> = Vec::new();
        for entry in entries {
            let entry = entry?;
            let device_id = entry.file_name().to_string_lossy().into_owned();
            if device_id.starts_with('.') || !entry.file_type()?.is_dir() {
                continue;
            }
            let backup = Backup {
                dir: entry.path(),
                device_id,
            };
            match backup.metadata() {
                Ok(metadata) => candidates.push((
                    metadata.session_finish_date.or(metadata.session_start_date),
                    backup,
                )),
                Err(error) => tracing::warn!(
                    dir = %backup.dir.display(),
                    %error,
                    "ignoring backup dir without readable metadata.json"
                ),
            }
        }

        // `None` sorts last under reverse ordering, so a dir with no session dates loses.
        candidates.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.device_id.cmp(&b.1.device_id))
        });

        let mut candidates = candidates.into_iter();
        let (_, chosen) = candidates.next().ok_or_else(|| {
            anyhow!(
                "no Arc device backup found in {} (expected Backup/<device-uuid>/metadata.json)",
                root.display()
            )
        })?;
        for (session, other) in candidates {
            tracing::warn!(
                device_id = %other.device_id,
                ?session,
                chosen = %chosen.device_id,
                "ignoring older Arc device backup"
            );
        }
        Ok(chosen)
    }

    pub fn metadata(&self) -> Result<Metadata> {
        let path = self.dir.join("metadata.json");
        let file = File::open(&path).with_context(|| format!("opening {}", path.display()))?;
        serde_json::from_reader(BufReader::new(file))
            .with_context(|| format!("parsing {}", path.display()))
    }

    pub fn place_files(&self) -> Result<Vec<BucketFile>> {
        self.bucket_files(BucketKind::Places)
    }

    pub fn item_files(&self) -> Result<Vec<BucketFile>> {
        self.bucket_files(BucketKind::Items)
    }

    pub fn sample_files(&self) -> Result<Vec<BucketFile>> {
        self.bucket_files(BucketKind::Samples)
    }

    fn bucket_files(&self, kind: BucketKind) -> Result<Vec<BucketFile>> {
        let dir = self.dir.join(kind.dir_name());
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            // A backup taken before any record of this kind existed simply has no directory.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(dir = %dir.display(), "Arc backup has no {} dir", kind.dir_name());
                return Ok(Vec::new());
            }
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", dir.display()));
            }
        };

        let mut files = Vec::new();
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                // iCloud replaces an evicted file with `.<name>.icloud`. Skipping it silently
                // would look like the bucket vanished, so make eviction visible instead.
                if name.ends_with(".icloud") {
                    tracing::warn!(
                        path = %entry.path().display(),
                        "skipping iCloud-evicted bucket placeholder"
                    );
                }
                continue;
            }
            if !(name.ends_with(".json") || name.ends_with(".json.gz")) {
                continue;
            }
            let metadata = entry.metadata()?;
            if !metadata.is_file() {
                continue;
            }
            files.push(BucketFile {
                path: entry.path(),
                kind,
                mtime: metadata.modified()?,
                len: metadata.len(),
            });
        }
        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }
}

pub fn read_places(path: &Path) -> Result<Vec<Place>> {
    read_bucket(path)
}

pub fn read_items(path: &Path) -> Result<Vec<TimelineItem>> {
    read_bucket(path)
}

pub fn read_samples(path: &Path) -> Result<Vec<LocomotionSample>> {
    read_bucket(path)
}

/// Streams straight from the file (through gzip when needed) rather than buffering the whole
/// bucket as a String first; sample weeks run to tens of megabytes.
fn read_bucket<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let reader = BufReader::new(file);
    let name = path.file_name().unwrap_or_default().to_string_lossy();

    if name.ends_with(".gz") {
        // MultiGzDecoder handles concatenated gzip members; plain GzDecoder stops after the first.
        // The second BufReader matters: serde_json reads in small chunks.
        serde_json::from_reader(BufReader::new(MultiGzDecoder::new(reader)))
    } else if name.ends_with(".json") {
        serde_json::from_reader(reader)
    } else {
        bail!("{} is not a .json or .json.gz bucket file", path.display());
    }
    .with_context(|| format!("parsing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::arc::enums::{ActivityType, MovingState, RecordingState};

    const NEWER_DEVICE: &str = "11111111-2222-4333-8444-555555555555";

    fn fixture_arc_dir() -> PathBuf {
        PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/backup"
        ))
    }

    fn fixture_backup() -> Backup {
        Backup::discover(&fixture_arc_dir()).unwrap()
    }

    #[test]
    fn discover_picks_the_newest_session() {
        let backup = fixture_backup();
        assert_eq!(backup.device_id, NEWER_DEVICE);
    }

    #[test]
    fn discover_errors_without_a_device_dir() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("Backup")).unwrap();
        let error = Backup::discover(temp.path()).unwrap_err().to_string();
        assert!(error.contains("no Arc device backup found"), "{error}");
    }

    #[test]
    fn metadata_parses() {
        let metadata = fixture_backup().metadata().unwrap();
        assert_eq!(metadata.schema_version, "2.4.0");
        assert_eq!(metadata.export_type.as_deref(), Some("incremental"));
        assert_eq!(metadata.stats.unwrap().place_count, Some(2));
        assert_eq!(
            metadata.last_backup_date,
            Some("2025-06-10T10:00:00Z".parse().unwrap())
        );
        assert!(metadata.app_metadata.is_some());
        assert!(metadata.backup_progress_date.is_none());
    }

    #[test]
    fn bucket_listings_cover_json_and_gz() {
        let backup = fixture_backup();
        let names = |files: Vec<BucketFile>| {
            files
                .into_iter()
                .map(|file| {
                    file.path
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>()
        };

        assert_eq!(names(backup.place_files().unwrap()), ["A.json"]);
        assert_eq!(names(backup.item_files().unwrap()), ["2025-06.json"]);
        assert_eq!(
            names(backup.sample_files().unwrap()),
            ["2025-W24.json.gz", "2025-W25.json"]
        );
        assert!(backup.place_files().unwrap()[0].len > 0);
    }

    #[test]
    fn icloud_placeholders_and_hidden_files_are_skipped() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("Backup").join(NEWER_DEVICE);
        fs::create_dir_all(dir.join("samples")).unwrap();
        fs::copy(
            fixture_arc_dir()
                .join("Backup")
                .join(NEWER_DEVICE)
                .join("metadata.json"),
            dir.join("metadata.json"),
        )
        .unwrap();
        fs::write(dir.join("samples").join("2025-W24.json"), "[]").unwrap();
        fs::write(dir.join("samples").join(".2025-W25.json.gz.icloud"), "").unwrap();
        fs::write(dir.join("samples").join(".DS_Store"), "").unwrap();
        fs::write(dir.join("samples").join("notes.txt"), "").unwrap();

        let files = Backup::discover(temp.path())
            .unwrap()
            .sample_files()
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].path.ends_with("2025-W24.json"));
    }

    #[test]
    fn places_parse_with_nulls_and_unknown_fields() {
        let backup = fixture_backup();
        let places = read_places(&backup.place_files().unwrap()[0].path).unwrap();

        assert_eq!(places.len(), 2);
        let station = &places[0];
        assert_eq!(station.country_code.as_deref(), Some("de"));
        assert_eq!(station.locality.as_deref(), Some("Dresden"));
        assert_eq!(station.seconds_from_gmt, Some(7200));
        assert_eq!(station.radius_sd, Some(18.25));
        assert!(places[1].last_visit_date.is_none());
        assert!(places[1].google_place_id.is_none());
    }

    #[test]
    fn items_parse_and_expose_helpers() {
        let backup = fixture_backup();
        let items = read_items(&backup.item_files().unwrap()[0].path).unwrap();

        // Five on the 10th, plus the flight of the 12th with a visit either side of it.
        assert_eq!(items.len(), 9);
        assert!(items[0].is_visit());
        assert_eq!(items[0].activity_type(), None);
        assert_eq!(
            items[0].base.start_date,
            "2025-06-10T08:00:00Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(items[0].visit.as_ref().unwrap().radius_sd, Some(15.0));
        assert_eq!(items[1].activity_type(), Some(ActivityType::Tram));
        assert!(items[1].activity_type().unwrap().is_moving_type());
        assert_eq!(items[3].activity_type(), Some(ActivityType::Other(99)));
        // The walk was never confirmed, so the classifier's guess is all there is.
        assert_eq!(items[4].activity_type(), Some(ActivityType::Walking));
    }

    #[test]
    fn samples_parse_from_gzip_and_plain_json() {
        let backup = fixture_backup();
        let files = backup.sample_files().unwrap();

        let gz = read_samples(&files[0].path).unwrap();
        assert_eq!(gz.len(), 15);
        assert_eq!(gz[0].moving_state, Some(MovingState::Moving));
        assert_eq!(gz[0].recording_state, Some(RecordingState::Recording));
        assert_eq!(gz[0].classified_activity_type, Some(ActivityType::Tram));
        assert_eq!(
            gz[0].date,
            "2025-06-10T08:20:00Z".parse::<Timestamp>().unwrap()
        );
        assert_eq!(gz[0].seconds_from_gmt, Some(7200));
        assert!(gz[9].heart_rate.is_none());

        let plain = read_samples(&files[1].path).unwrap();
        assert_eq!(plain.len(), 8);
        assert_eq!(
            plain[1].confirmed_activity_type,
            Some(ActivityType::Other(99))
        );
        assert!(plain[1].latitude.is_none());
        assert!(plain[1].seconds_from_gmt.is_none());
        assert_eq!(
            plain[3].confirmed_activity_type,
            Some(ActivityType::Airplane)
        );
    }

    #[test]
    fn reading_a_non_bucket_path_errors() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("places.txt");
        fs::write(&path, "[]").unwrap();
        assert!(read_places(&path).is_err());
    }
}
