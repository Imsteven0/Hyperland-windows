//! Background data sources. Each service owns a thread and reports
//! `Send` snapshots; the UI thread turns them into Slint values.
pub mod apps;
pub mod calc;
pub mod clipboard;
pub mod connectivity;
pub mod emoji;
pub mod icons;
pub mod komorebi;
pub mod media;
pub mod perf;
pub mod sysmon;
pub mod temps;
pub mod wallpaper_repo;
pub mod weather;
