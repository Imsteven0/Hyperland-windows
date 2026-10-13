//! Downloads a GitHub repository's images into the wallpaper folder
//! (Settings › Wallpaper). The git trees API gives the index (2 API calls),
//! raw.githubusercontent.com serves the files. Files already present with the
//! same size are skipped, so running it again resumes or picks up new images;
//! each file is written under a temporary name and renamed when complete.

use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

const IMAGE_EXTS: [&str; 5] = ["jpg", "jpeg", "png", "webp", "bmp"];
const WORKERS: usize = 4;

/// A repository (optionally a branch and sub-folder of it).
#[derive(Debug, Clone, PartialEq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    pub branch: Option<String>,
    pub subdir: String,
}

impl Repo {
    /// Accepts "https://github.com/owner/repo", ".../tree/branch/sub/dir" or "owner/repo".
    pub fn parse(url: &str) -> Option<Repo> {
        let s = url.trim().trim_end_matches('/');
        let s = s.strip_prefix("https://").or_else(|| s.strip_prefix("http://")).unwrap_or(s);
        let s = s.strip_prefix("www.").unwrap_or(s);
        let s = s.strip_prefix("github.com/").unwrap_or(s);
        let mut parts = s.split('/').filter(|p| !p.is_empty());
        let owner = parts.next()?;
        let name = parts.next()?.trim_end_matches(".git");
        // GitHub owners never contain dots: anything else is another host.
        if owner.contains(['.', ':']) || name.is_empty() {
            return None;
        }
        let rest: Vec<&str> = parts.collect();
        let (branch, subdir) = match rest.as_slice() {
            ["tree", branch, sub @ ..] => (Some((*branch).to_owned()), sub.join("/")),
            _ => (None, String::new()),
        };
        Some(Repo { owner: owner.to_owned(), name: name.to_owned(), branch, subdir })
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    Indexing,
    Running { done: usize, total: usize, bytes: u64, total_bytes: u64 },
    Finished { downloaded: usize, skipped: usize, failed: usize, cancelled: bool },
    Failed(String),
}

/// A running download; dropping it does not stop it, `cancel` does.
pub struct Download {
    cancel: Arc<AtomicBool>,
}

impl Download {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

struct Item {
    /// Path inside the repository (for the URL).
    path: String,
    /// Path below the requested sub-folder (on disk).
    rel: String,
    size: u64,
}

/// Starts downloading `repo`'s images into `dest`, reporting on a worker thread.
pub fn start(repo: Repo, dest: PathBuf, on: impl Fn(Progress) + Send + Sync + 'static) -> Download {
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    thread::Builder::new()
        .name("wallpaper-download".into())
        .spawn(move || {
            on(Progress::Indexing);
            match index(&repo) {
                Ok((branch, items)) => run(&repo, &branch, items, &dest, &flag, &on),
                Err(e) => {
                    log::warn!("wallpaper download: {e:#}");
                    on(Progress::Failed(format!("{e:#}")));
                }
            }
        })
        .expect("spawn wallpaper download");
    Download { cancel }
}

pub(crate) fn agent() -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::config::Config::builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(30)))
            .build(),
    )
}

pub(crate) fn get_json(agent: &ureq::Agent, url: &str) -> Result<Value> {
    let mut resp = agent
        .get(url)
        .header("User-Agent", "caelestia-win")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => anyhow!("repository not found"),
            ureq::Error::StatusCode(403 | 429) => anyhow!("GitHub rate limit reached, try again later"),
            e => anyhow!(e),
        })?;
    Ok(resp.body_mut().read_json::<Value>()?)
}

pub fn is_image(path: &str) -> bool {
    path.rsplit_once('.').is_some_and(|(_, ext)| IMAGE_EXTS.contains(&ext.to_ascii_lowercase().as_str()))
}

/// A file in a repository's git tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub path: String,
    pub size: u64,
}

/// Lists every file of the repository (2 API calls): the branch used and
/// its blobs. Shared by the wallpaper and the dots downloads.
pub fn tree(agent: &ureq::Agent, repo: &Repo) -> Result<(String, Vec<Blob>)> {
    let base = format!("https://api.github.com/repos/{}/{}", repo.owner, repo.name);
    let branch = match &repo.branch {
        Some(b) => b.clone(),
        None => get_json(agent, &base)?["default_branch"].as_str().context("repository has no default branch")?.to_owned(),
    };
    let tree = get_json(agent, &format!("{base}/git/trees/{}?recursive=1", encode_path(&branch)))?;
    if tree["truncated"].as_bool() == Some(true) {
        log::warn!("{}/{} is too large for one index, some files are missing", repo.owner, repo.name);
    }
    let blobs = tree["tree"]
        .as_array()
        .context("unexpected GitHub response")?
        .iter()
        .filter(|t| t["type"] == "blob")
        .filter_map(|t| Some(Blob { path: t["path"].as_str()?.to_owned(), size: t["size"].as_u64().unwrap_or(0) }))
        .collect();
    Ok((branch, blobs))
}

/// Direct download URL of a file on a branch.
pub fn raw_url(repo: &Repo, branch: &str, path: &str) -> String {
    format!("https://raw.githubusercontent.com/{}/{}/{}/{}", repo.owner, repo.name, encode_path(branch), encode_path(path))
}

/// Downloads a small text file (manifests, JSON), refusing anything over `max` bytes.
pub fn get_text(agent: &ureq::Agent, url: &str, max: u64) -> Result<String> {
    let mut resp = agent.get(url).header("User-Agent", "caelestia-win").call()?;
    let mut buf = Vec::new();
    resp.body_mut().as_reader().take(max + 1).read_to_end(&mut buf)?;
    if buf.len() as u64 > max {
        bail!("file too large");
    }
    Ok(String::from_utf8(buf)?)
}

fn index(repo: &Repo) -> Result<(String, Vec<Item>)> {
    let (branch, blobs) = tree(&agent(), repo)?;
    let prefix = if repo.subdir.is_empty() { String::new() } else { format!("{}/", repo.subdir) };
    let items: Vec<Item> = blobs
        .into_iter()
        .filter_map(|b| {
            let rel = b.path.strip_prefix(&prefix)?.to_owned();
            is_image(&rel).then_some(Item { rel, path: b.path, size: b.size })
        })
        .collect();
    if items.is_empty() {
        bail!("no images in that repository");
    }
    Ok((branch, items))
}

fn run(repo: &Repo, branch: &str, items: Vec<Item>, dest: &Path, cancel: &AtomicBool, on: &(impl Fn(Progress) + Sync)) {
    let total = items.len();
    let total_bytes: u64 = items.iter().map(|i| i.size).sum();
    let (next, done, bytes) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicU64::new(0));
    let (downloaded, skipped, failed) = (AtomicUsize::new(0), AtomicUsize::new(0), AtomicUsize::new(0));
    on(Progress::Running { done: 0, total, bytes: 0, total_bytes });
    thread::scope(|s| {
        for _ in 0..WORKERS {
            s.spawn(|| {
                let agent = agent();
                while !cancel.load(Ordering::Relaxed) {
                    let Some(item) = items.get(next.fetch_add(1, Ordering::Relaxed)) else { break };
                    let target = item.rel.split('/').fold(dest.to_path_buf(), |p, seg| p.join(seg));
                    let outcome = if fs::metadata(&target).is_ok_and(|m| m.len() == item.size) {
                        Ok(false)
                    } else {
                        let url = raw_url(repo, branch, &item.path);
                        // One retry: raw.githubusercontent.com drops the odd connection.
                        fetch(&agent, &url, &target, cancel).or_else(|_| fetch(&agent, &url, &target, cancel)).map(|_| true)
                    };
                    match outcome {
                        Ok(true) => downloaded.fetch_add(1, Ordering::Relaxed),
                        Ok(false) => skipped.fetch_add(1, Ordering::Relaxed),
                        Err(e) => {
                            if !cancel.load(Ordering::Relaxed) {
                                log::warn!("wallpaper download {}: {e:#}", item.path);
                            }
                            failed.fetch_add(1, Ordering::Relaxed)
                        }
                    };
                    let b = bytes.fetch_add(item.size, Ordering::Relaxed) + item.size;
                    let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                    on(Progress::Running { done: d, total, bytes: b, total_bytes });
                }
            });
        }
    });
    let cancelled = cancel.load(Ordering::Relaxed);
    let (downloaded, skipped, failed) = (downloaded.into_inner(), skipped.into_inner(), failed.into_inner());
    // Cancelled items count as failed above; don't report them as errors.
    let failed = if cancelled { 0 } else { failed };
    log::info!("wallpaper download {}/{}: {downloaded} new, {skipped} present, {failed} failed{}", repo.owner, repo.name, if cancelled { " (cancelled)" } else { "" });
    on(Progress::Finished { downloaded, skipped, failed, cancelled });
}

fn fetch(agent: &ureq::Agent, url: &str, target: &Path, cancel: &AtomicBool) -> Result<()> {
    if let Some(dir) = target.parent() {
        fs::create_dir_all(dir)?;
    }
    let part = target.with_extension(format!("{}.part", target.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let result = (|| -> Result<()> {
        let mut resp = agent.get(url).header("User-Agent", "caelestia-win").call()?;
        let mut reader = resp.body_mut().as_reader();
        let mut file = fs::File::create(&part)?;
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])?;
        }
        file.flush()?;
        drop(file);
        fs::rename(&part, target)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&part);
    }
    result
}

/// Percent-encodes each path segment (file names with spaces, accents...).
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_repo_urls() {
        let r = Repo::parse("https://github.com/Ajaymanikandan0x/hyprland_wallpapers").unwrap();
        assert_eq!((r.owner.as_str(), r.name.as_str(), r.branch, r.subdir.as_str()), ("Ajaymanikandan0x", "hyprland_wallpapers", None, ""));
        let r = Repo::parse("github.com/o/r.git/").unwrap();
        assert_eq!(r.name, "r");
        let r = Repo::parse("https://github.com/o/r/tree/main/Dynamic-Wallpapers/Dark").unwrap();
        assert_eq!((r.branch.as_deref(), r.subdir.as_str()), (Some("main"), "Dynamic-Wallpapers/Dark"));
        assert_eq!(Repo::parse("o/r").unwrap().owner, "o");
        assert!(Repo::parse("https://gitlab.com/o/r").is_none());
        assert!(Repo::parse("just-a-word").is_none());
    }

    /// Network: `cargo test -- --ignored downloads_a_folder`.
    #[test]
    #[ignore]
    fn downloads_a_folder() {
        let dest = std::env::temp_dir().join("caelestia-wallpaper-test");
        let _ = fs::remove_dir_all(&dest);
        let repo = Repo::parse("https://github.com/Ajaymanikandan0x/hyprland_wallpapers/tree/master/Dynamic-Wallpapers/Dark").unwrap();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        start(repo.clone(), dest.clone(), move |p| tx.lock().unwrap().send(p).unwrap());
        let last = rx.iter().find(|p| matches!(p, Progress::Finished { .. } | Progress::Failed(_))).unwrap();
        assert!(matches!(last, Progress::Finished { downloaded: 7, failed: 0, .. }), "{last:?}");
        assert_eq!(fs::read_dir(&dest).unwrap().count(), 7);
        // A second run finds everything in place.
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = std::sync::Mutex::new(tx);
        start(repo, dest.clone(), move |p| tx.lock().unwrap().send(p).unwrap());
        let last = rx.iter().find(|p| matches!(p, Progress::Finished { .. } | Progress::Failed(_))).unwrap();
        assert!(matches!(last, Progress::Finished { downloaded: 0, skipped: 7, .. }), "{last:?}");
        let _ = fs::remove_dir_all(&dest);
    }

    #[test]
    fn encodes_paths() {
        assert_eq!(encode_path("a b/ñ.png"), "a%20b/%C3%B1.png");
    }
}
