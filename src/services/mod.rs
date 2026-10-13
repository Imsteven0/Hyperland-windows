//! Background data sources. Each service owns a thread and reports
//! `Send` snapshots; the UI thread turns them into Slint values.
pub mod apps;
pub mod connectivity;
pub mod icons;
pub mod komorebi;
pub mod media;
pub mod sysmon;
pub mod temps;
pub mod wallpaper_repo;
pub mod weather;
