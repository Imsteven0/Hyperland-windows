//! Update notice: asks GitHub for the latest release (shortly after start,
//! then once a day) and tells Settings › About when it is newer than the
//! running build. Nothing is downloaded or installed automatically, and
//! `general.checkUpdates: false` turns the request off entirely.

use std::{cmp::Ordering, thread, time::Duration};

use anyhow::{Context, Result};
use crossbeam_channel::{RecvTimeoutError, Sender};
use serde_json::Value;

pub const REPO: &str = "Imsteven0/Hyperland-windows";

/// A semantic version (`1.2.3`, `v1.2`, `1.0.0-beta.2`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Pre-release identifiers ("beta.2" -> ["beta", "2"]); empty for a release.
    pub pre: Vec<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let s = s.split('+').next()?; // build metadata never matters
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) => (c, p.split('.').filter(|p| !p.is_empty()).map(str::to_owned).collect()),
            None => (s, Vec::new()),
        };
        let mut nums = core.split('.').map(|n| n.parse::<u64>());
        let major = nums.next()?.ok()?;
        let minor = nums.next().unwrap_or(Ok(0)).ok()?;
        let patch = nums.next().unwrap_or(Ok(0)).ok()?;
        if nums.next().is_some() {
            return None;
        }
        Some(Self { major, minor, patch, pre })
    }
}

impl Ord for Version {
    /// Semver precedence: 1.0.0-alpha < 1.0.0-alpha.1 < 1.0.0-beta < 1.0.0.
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch)).then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => {
                for (a, b) in self.pre.iter().zip(&other.pre) {
                    let ord = match (a.parse::<u64>(), b.parse::<u64>()) {
                        (Ok(x), Ok(y)) => x.cmp(&y),
                        (Ok(_), Err(_)) => Ordering::Less,
                        (Err(_), Ok(_)) => Ordering::Greater,
                        (Err(_), Err(_)) => a.cmp(b),
                    };
                    if ord != Ordering::Equal {
                        return ord;
                    }
                }
                self.pre.len().cmp(&other.pre.len())
            }
        })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Release {
    pub version: Version,
    pub tag: String,
    /// The release page.
    pub page: String,
    /// The installer asset, when the release has one.
    pub installer: Option<String>,
}

/// Reads GitHub's `releases/latest` response.
pub fn parse_release(v: &Value) -> Option<Release> {
    let tag = v["tag_name"].as_str()?.to_owned();
    let version = Version::parse(&tag)?;
    let installer = v["assets"].as_array().into_iter().flatten().filter_map(|a| a["browser_download_url"].as_str()).find(|u| u.to_lowercase().ends_with("-setup.exe")).map(str::to_owned);
    Some(Release { version, tag, page: v["html_url"].as_str().unwrap_or_default().to_owned(), installer })
}

/// What a check found.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    UpToDate,
    Available(Release),
    Failed(String),
}

/// Compares the latest release with `current` (the running version).
pub fn evaluate(latest: Option<Release>, current: &str) -> Check {
    match (latest, Version::parse(current)) {
        (Some(r), Some(cur)) if r.version > cur => Check::Available(r),
        (Some(_), Some(_)) => Check::UpToDate,
        (None, _) => Check::Failed("unexpected GitHub response".into()),
        (_, None) => Check::Failed(format!("bad current version {current}")),
    }
}

/// One blocking check (the background loop and the "Check now" button).
pub fn check_once(current: &str) -> Check {
    match fetch_latest() {
        Ok(Some(r)) => evaluate(Some(r), current),
        Ok(None) => Check::UpToDate,
        Err(e) => {
            log::info!("update check: {e:#}");
            Check::Failed(format!("{e:#}"))
        }
    }
}

fn fetch_latest() -> Result<Option<Release>> {
    let agent = super::wallpaper_repo::agent();
    let v = match super::wallpaper_repo::get_json(&agent, &format!("https://api.github.com/repos/{REPO}/releases/latest")) {
        Ok(v) => v,
        // No release published yet.
        Err(e) if e.to_string() == "repository not found" => return Ok(None),
        Err(e) => return Err(e).context("checking for updates"),
    };
    Ok(parse_release(&v))
}

/// Checks in the background; `check_now` forces a check (Settings button).
pub struct UpdateChecker {
    tx: Sender<()>,
}

impl UpdateChecker {
    pub fn spawn(current: &'static str, first_delay: Duration, every: Duration, on: impl Fn(Check) + Send + 'static) -> Self {
        let (tx, rx) = crossbeam_channel::unbounded::<()>();
        thread::Builder::new()
            .name("updates".into())
            .spawn(move || {
                let mut wait = first_delay;
                loop {
                    match rx.recv_timeout(wait) {
                        Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                        Err(RecvTimeoutError::Disconnected) => return,
                    }
                    on(check_once(current));
                    wait = every;
                }
            })
            .expect("spawn update checker");
        Self { tx }
    }

    pub fn check_now(&self) {
        let _ = self.tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap_or_else(|| panic!("{s}"))
    }

    #[test]
    fn parses_versions() {
        assert_eq!(v("v1.2.3"), Version { major: 1, minor: 2, patch: 3, pre: vec![] });
        assert_eq!(v("0.4"), v("0.4.0"));
        assert_eq!(v("1.0.0-beta.2").pre, ["beta", "2"]);
        assert_eq!(v("1.0.0+build.7"), v("1.0.0"));
        assert!(Version::parse("latest").is_none());
        assert!(Version::parse("1.2.3.4").is_none());
        assert!(Version::parse("").is_none());
    }

    #[test]
    fn orders_like_semver() {
        let order = ["0.9.9", "1.0.0-alpha", "1.0.0-alpha.1", "1.0.0-alpha.beta", "1.0.0-beta", "1.0.0-beta.2", "1.0.0-beta.11", "1.0.0-rc.1", "1.0.0", "1.0.1", "1.10.0", "2.0.0"];
        for pair in order.windows(2) {
            assert!(v(pair[0]) < v(pair[1]), "{} < {}", pair[0], pair[1]);
        }
    }

    #[test]
    fn reads_the_latest_release() {
        let body = json!({
            "tag_name": "v0.2.0",
            "html_url": "https://github.com/Imsteven0/Hyperland-windows/releases/tag/v0.2.0",
            "assets": [
                { "browser_download_url": "https://x/caelestia-win-0.2.0-x64.zip" },
                { "browser_download_url": "https://x/caelestia-win-0.2.0-setup.exe" }
            ]
        });
        let r = parse_release(&body).unwrap();
        assert_eq!(r.version, v("0.2.0"));
        assert_eq!(r.installer.as_deref(), Some("https://x/caelestia-win-0.2.0-setup.exe"));
        assert!(parse_release(&json!({ "message": "Not Found" })).is_none());
    }

    #[test]
    fn decides_whether_to_tell_the_user() {
        let rel = |t: &str| parse_release(&json!({ "tag_name": t })).unwrap();
        assert!(matches!(evaluate(Some(rel("v0.2.0")), "0.1.0"), Check::Available(_)));
        assert_eq!(evaluate(Some(rel("v0.1.0")), "0.1.0"), Check::UpToDate);
        assert_eq!(evaluate(Some(rel("v0.1.0")), "0.2.0-dev"), Check::UpToDate, "a newer dev build never sees an 'update'");
        assert!(matches!(evaluate(Some(rel("v0.1.0")), "garbage"), Check::Failed(_)));
        // The running package version always parses.
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }
}
