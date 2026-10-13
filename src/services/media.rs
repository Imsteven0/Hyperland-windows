//! Now-playing information through Windows' Global System Media Transport
//! Controls (the same source as the volume flyout): works with Spotify,
//! browsers, the Media Player app, etc.

use std::{
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use crossbeam_channel::{Receiver, Sender, unbounded};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use windows::{
    Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status},
    Storage::Streams::DataReader,
};

#[derive(Debug, Clone, Copy)]
pub enum MediaCommand {
    Toggle,
    Next,
    Previous,
    /// Jump to a fraction (0..1) of the current track.
    Seek(f64),
}

#[derive(Clone, Default)]
pub struct MediaSnapshot {
    pub available: bool,
    pub playing: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub position: f64,
    pub length: f64,
    /// `Some` only when the track changed (cover is re-sent sparingly).
    pub cover: Option<Option<SharedPixelBuffer<Rgba8Pixel>>>,
}

pub struct Media {
    tx: Sender<MediaCommand>,
}

impl Media {
    pub fn spawn(interval: Duration, on_update: impl Fn(MediaSnapshot) + Send + 'static) -> Self {
        let (tx, rx) = unbounded();
        thread::Builder::new()
            .name("media".into())
            .spawn(move || {
                crate::platform::com_init(false);
                run(rx, interval, on_update)
            })
            .expect("spawn media");
        Self { tx }
    }

    pub fn send(&self, cmd: MediaCommand) {
        let _ = self.tx.send(cmd);
    }
}

fn run(rx: Receiver<MediaCommand>, interval: Duration, on_update: impl Fn(MediaSnapshot)) {
    let manager = loop {
        match Manager::RequestAsync().and_then(|op| op.join()) {
            Ok(m) => break m,
            Err(e) => {
                log::warn!("media session manager unavailable: {e}");
                thread::sleep(Duration::from_secs(10));
            }
        }
    };
    let mut last_track = String::new();
    let mut last: Option<(bool, String, f64)> = None;
    loop {
        match rx.recv_timeout(interval) {
            Ok(cmd) => {
                if let Ok(s) = manager.GetCurrentSession() {
                    let r = match cmd {
                        MediaCommand::Toggle => s.TryTogglePlayPauseAsync(),
                        MediaCommand::Next => s.TrySkipNextAsync(),
                        MediaCommand::Previous => s.TrySkipPreviousAsync(),
                        MediaCommand::Seek(f) => s.GetTimelineProperties().and_then(|tl| {
                            let (start, end) = (tl.StartTime()?.Duration, tl.EndTime()?.Duration);
                            s.TryChangePlaybackPositionAsync(start + ((end - start) as f64 * f.clamp(0.0, 1.0)) as i64)
                        }),
                    };
                    let _ = r.and_then(|op| op.join());
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            Err(_) => {}
        }

        let mut snap = snapshot(&manager).unwrap_or_default();
        let track = format!("{}\u{1}{}\u{1}{}", snap.title, snap.artist, snap.app);
        if track != last_track {
            last_track = track;
            snap.cover = Some(manager.GetCurrentSession().ok().and_then(|s| cover(&s)));
        }
        let key = (snap.playing, last_track.clone(), snap.position.floor());
        if snap.cover.is_none() && last.as_ref() == Some(&key) {
            continue; // nothing visible changed
        }
        last = Some(key);
        on_update(snap);
    }
}

fn snapshot(manager: &Manager) -> windows::core::Result<MediaSnapshot> {
    let s = manager.GetCurrentSession()?;
    let props = s.TryGetMediaPropertiesAsync()?.join()?;
    let playing = s.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing;
    let tl = s.GetTimelineProperties()?;
    let to_secs = |ticks: i64| ticks as f64 / 10_000_000.0;
    let mut position = to_secs(tl.Position()?.Duration);
    let length = to_secs(tl.EndTime()?.Duration - tl.StartTime()?.Duration);
    if playing {
        // Apps only update the timeline occasionally; extrapolate like Caelestia does.
        let updated = tl.LastUpdatedTime()?.UniversalTime; // 100ns since 1601
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos() as i64 / 100 + 116_444_736_000_000_000;
        if updated > 0 && now > updated {
            position += to_secs(now - updated);
        }
    }
    let app = pretty_app(&s.SourceAppUserModelId()?.to_string());
    Ok(MediaSnapshot {
        available: true,
        playing,
        title: props.Title()?.to_string(),
        artist: props.Artist()?.to_string(),
        album: props.AlbumTitle()?.to_string(),
        app,
        position: if length > 0.0 { position.min(length) } else { position },
        length,
        cover: None,
    })
}

fn pretty_app(aumid: &str) -> String {
    let base = aumid.rsplit('!').next().unwrap_or(aumid);
    let base = base.trim_end_matches(".exe").trim_end_matches(".EXE");
    let base = base.rsplit('.').next().unwrap_or(base);
    let mut c = base.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn cover(s: &Session) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    let props = s.TryGetMediaPropertiesAsync().ok()?.join().ok()?;
    let stream = props.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.join().ok()?;
    let mut bytes = vec![0u8; size as usize];
    reader.ReadBytes(&mut bytes).ok()?;
    let img = image::load_from_memory(&bytes).ok()?.thumbnail(256, 256).into_rgba8();
    Some(SharedPixelBuffer::clone_from_slice(img.as_raw(), img.width(), img.height()))
}

pub fn fmt_time(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}
