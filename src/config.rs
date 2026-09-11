use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

/// OpenFreeMap serves this for free and without a key.
pub const DEFAULT_MAP_STYLE: &str = "https://tiles.openfreemap.org/styles/liberty";
const DEFAULT_ARC_DIR: &str =
    "~/Library/Mobile Documents/iCloud~com~bigpaua~Arc-Timeline-Editor/Documents";
const DEFAULT_DATA_DIR: &str = "~/Library/Application Support/arches";
const DEFAULT_BIND: &str = "127.0.0.1:8471";
const DEFAULT_INGEST_INTERVAL: &str = "15m";

pub struct Config {
    /// Arc's iCloud `Documents` dir. Read-only, without exception: see PLAN.md.
    pub arc_dir: PathBuf,
    pub data_dir: PathBuf,
    pub bind: String,
    pub map_style: String,
    pub ingest_interval: Duration,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_pairs(std::env::vars())
    }

    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("arches.db")
    }

    /// Core loader, parameterized over an env-like source instead of touching the process
    /// environment directly, so tests can exercise overrides without racing global state.
    pub fn from_pairs<I, K, V>(pairs: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let vars: HashMap<String, String> = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        let get = |key: &str| vars.get(key).cloned();
        let home = get("HOME");

        Ok(Self {
            arc_dir: expand_home(
                &get("ARCHES_ARC_DIR").unwrap_or_else(|| DEFAULT_ARC_DIR.to_string()),
                home.as_deref(),
            ),
            data_dir: expand_home(
                &get("ARCHES_DATA_DIR").unwrap_or_else(|| DEFAULT_DATA_DIR.to_string()),
                home.as_deref(),
            ),
            // Loopback only: there is no auth, exposing it is a deliberate deployment choice.
            bind: get("ARCHES_BIND").unwrap_or_else(|| DEFAULT_BIND.to_string()),
            map_style: get("ARCHES_MAP_STYLE").unwrap_or_else(|| DEFAULT_MAP_STYLE.to_string()),
            ingest_interval: parse_interval(
                &get("ARCHES_INGEST_INTERVAL")
                    .unwrap_or_else(|| DEFAULT_INGEST_INTERVAL.to_string()),
            )?,
        })
    }
}

/// `~` only expands as a leading path component, matching shell behavior; anything else is
/// passed through so an already-absolute override is never rewritten.
fn expand_home(path: &str, home: Option<&str>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => PathBuf::from(home).join(rest),
        _ => PathBuf::from(path),
    }
}

/// Arc rewrites whole buckets retroactively, so polling is cheap insurance against missed
/// FSEvents; the interval just needs a human-friendly unit, not sub-second precision.
fn parse_interval(raw: &str) -> anyhow::Result<Duration> {
    let raw = raw.trim();
    let split_at = raw.len().saturating_sub(1);
    let (number, unit) = raw.split_at(split_at);
    let value: u64 = number.parse().map_err(|_| {
        anyhow::anyhow!("invalid ingest interval {raw:?}: expected a number followed by s, m or h")
    })?;
    let seconds = match unit {
        "s" => value,
        "m" => value * 60,
        "h" => value * 3600,
        _ => anyhow::bail!(
            "invalid ingest interval {raw:?}: expected a number followed by s, m or h"
        ),
    };
    Ok(Duration::from_secs(seconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> Config {
        Config::from_pairs(pairs.iter().copied()).unwrap()
    }

    #[test]
    fn defaults_expand_home_and_parse_interval() {
        let config = config(&[("HOME", "/home/kilian")]);
        assert_eq!(
            config.arc_dir,
            PathBuf::from(
                "/home/kilian/Library/Mobile Documents/iCloud~com~bigpaua~Arc-Timeline-Editor/Documents"
            )
        );
        assert_eq!(
            config.data_dir,
            PathBuf::from("/home/kilian/Library/Application Support/arches")
        );
        assert_eq!(config.bind, "127.0.0.1:8471");
        assert_eq!(
            config.map_style,
            "https://tiles.openfreemap.org/styles/liberty"
        );
        assert_eq!(config.ingest_interval, Duration::from_secs(15 * 60));
    }

    #[test]
    fn overrides_win_over_defaults() {
        let config = config(&[
            ("HOME", "/home/kilian"),
            ("ARCHES_ARC_DIR", "/custom/arc"),
            ("ARCHES_DATA_DIR", "/custom/data"),
            ("ARCHES_BIND", "0.0.0.0:9000"),
            ("ARCHES_MAP_STYLE", "https://example.com/style.json"),
            ("ARCHES_INGEST_INTERVAL", "30s"),
        ]);
        assert_eq!(config.arc_dir, PathBuf::from("/custom/arc"));
        assert_eq!(config.data_dir, PathBuf::from("/custom/data"));
        assert_eq!(config.bind, "0.0.0.0:9000");
        assert_eq!(config.map_style, "https://example.com/style.json");
        assert_eq!(config.ingest_interval, Duration::from_secs(30));
    }

    #[test]
    fn parses_seconds_minutes_and_hours() {
        assert_eq!(parse_interval("45s").unwrap(), Duration::from_secs(45));
        assert_eq!(parse_interval("2m").unwrap(), Duration::from_secs(120));
        assert_eq!(parse_interval("3h").unwrap(), Duration::from_secs(3 * 3600));
    }

    #[test]
    fn rejects_invalid_intervals() {
        assert!(parse_interval("").is_err());
        assert!(parse_interval("10").is_err());
        assert!(parse_interval("10x").is_err());
        assert!(parse_interval("abcm").is_err());
    }
}
