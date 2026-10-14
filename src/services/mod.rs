//! Background data sources. Each service owns a thread and reports
//! `Send` snapshots; the UI thread turns them into Slint values.
pub mod apps;
pub mod calc;
pub mod clipboard;
pub mod confetti;
pub mod connectivity;
pub mod dots_repo;
pub mod emoji;
pub mod icons;
pub mod komorebi;
pub mod media;
pub mod notifications;
pub mod perf;
pub mod plugins;
pub mod sysmon;
pub mod temps;
pub mod updates;
pub mod wallpaper_repo;
pub mod weather;
pub mod workspace_anim;
