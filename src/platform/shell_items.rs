//! Start-menu application index (the `shell:AppsFolder` virtual folder, which
//! covers Win32 and Store apps alike) and high-resolution icon extraction.

use std::{collections::HashMap, ffi::c_void, mem::size_of, path::Path};

use slint::{Rgba8Pixel, SharedPixelBuffer};
use windows::{
    Win32::{
        Foundation::SIZE,
        Graphics::Gdi::{BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ},
        System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ},
        UI::Shell::{
            BHID_EnumItems, FOLDERID_AppsFolder, IEnumShellItems, IShellItem, IShellItemImageFactory, IShellLinkW, KF_FLAG_DEFAULT, SHCreateItemFromParsingName,
            SHGetKnownFolderItem, SIGDN, SIGDN_NORMALDISPLAY, SIGDN_PARENTRELATIVEPARSING, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY, ShellLink,
        },
    },
    core::{HSTRING, Interface},
};

#[derive(Clone)]
pub struct AppRecord {
    pub name: String,
    pub description: String,
    /// Passed to ShellExecute as `shell:AppsFolder\<launch>`.
    pub launch: String,
    pub icon: Option<SharedPixelBuffer<Rgba8Pixel>>,
}

fn display_name(item: &IShellItem, kind: SIGDN) -> Option<String> {
    unsafe {
        let p = item.GetDisplayName(kind).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

/// Enumerates installed apps. Must run on an STA thread.
pub fn enumerate_apps(hidden: &[String], icon_px: i32) -> Vec<AppRecord> {
    let descriptions = start_menu_descriptions();
    let mut out = Vec::new();
    unsafe {
        let Ok(folder) = SHGetKnownFolderItem::<IShellItem>(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, None) else { return out };
        let Ok(items) = folder.BindToHandler::<_, IEnumShellItems>(None, &BHID_EnumItems) else { return out };
        loop {
            let mut batch: [Option<IShellItem>; 1] = [None];
            let mut fetched = 0u32;
            if items.Next(&mut batch, Some(&mut fetched)).is_err() || fetched == 0 {
                break;
            }
            let Some(item) = batch[0].take() else { break };
            let (Some(name), Some(launch)) = (display_name(&item, SIGDN_NORMALDISPLAY), display_name(&item, SIGDN_PARENTRELATIVEPARSING)) else { continue };
            let lower = name.to_lowercase();
            if lower.contains("uninstall") || lower.contains("desinstalar") || launch.starts_with("http") || hidden.iter().any(|h| h.eq_ignore_ascii_case(&name) || *h == launch) {
                continue;
            }
            let description = descriptions.get(&lower).cloned().unwrap_or_default();
            let icon = item_icon(&item, icon_px);
            out.push(AppRecord { name, description, launch, icon });
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// `.lnk` descriptions ("Comment" in .desktop terms) keyed by lowercase name.
fn start_menu_descriptions() -> HashMap<String, String> {
    let mut map = HashMap::new();
    let roots = [
        std::env::var("ProgramData").map(|p| Path::new(&p).join(r"Microsoft\Windows\Start Menu\Programs")),
        std::env::var("APPDATA").map(|p| Path::new(&p).join(r"Microsoft\Windows\Start Menu\Programs")),
    ];
    let Ok(link): Result<IShellLinkW, _> = (unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }) else { return map };
    let Ok(persist) = link.cast::<IPersistFile>() else { return map };
    let mut stack: Vec<_> = roots.into_iter().flatten().collect();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("lnk") {
                continue;
            }
            unsafe {
                if persist.Load(&HSTRING::from(path.as_os_str()), STGM_READ).is_err() {
                    continue;
                }
                let mut buf = [0u16; 512];
                if link.GetDescription(&mut buf).is_ok() {
                    let desc = String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]);
                    if !desc.is_empty() && !desc.starts_with('@') {
                        if let Some(stem) = path.file_stem() {
                            map.insert(stem.to_string_lossy().to_lowercase(), desc);
                        }
                    }
                }
            }
        }
    }
    map
}

/// Icon/thumbnail for any path (used for wallpapers and recordings too).
pub fn path_thumbnail(path: &Path, px: i32, icon_only: bool) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path.as_os_str()), None).ok()?;
        if icon_only { item_icon(&item, px) } else { item_image(&item, px, SIIGBF_BIGGERSIZEOK) }
    }
}

fn item_icon(item: &IShellItem, px: i32) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    item_image(item, px, SIIGBF_ICONONLY)
}

fn item_image(item: &IShellItem, px: i32, flags: windows::Win32::UI::Shell::SIIGBF) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let hbmp = factory.GetImage(SIZE { cx: px, cy: px }, flags).ok()?;
        let buf = hbitmap_to_rgba(hbmp);
        let _ = DeleteObject(HGDIOBJ(hbmp.0));
        buf
    }
}

/// Converts a 32bpp HBITMAP (premultiplied BGRA) into a premultiplied RGBA buffer.
unsafe fn hbitmap_to_rgba(hbmp: HBITMAP) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
    unsafe {
        let mut bm = BITMAP::default();
        if GetObjectW(HGDIOBJ(hbmp.0), size_of::<BITMAP>() as i32, Some(&mut bm as *mut _ as *mut c_void)) == 0 {
            return None;
        }
        let (w, h) = (bm.bmWidth, bm.bmHeight.abs());
        if w <= 0 || h <= 0 {
            return None;
        }
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let rows = GetDIBits(dc, hbmp, 0, h as u32, Some(pixels.as_mut_ptr() as *mut c_void), &mut info, DIB_RGB_COLORS);
        let _ = DeleteDC(dc);
        if rows == 0 {
            return None;
        }
        // Some icons come without alpha (all zero): treat them as opaque.
        let has_alpha = pixels.chunks_exact(4).any(|p| p[3] != 0);
        let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(w as u32, h as u32);
        for (dst, src) in buf.make_mut_slice().iter_mut().zip(pixels.chunks_exact(4)) {
            *dst = Rgba8Pixel { r: src[2], g: src[1], b: src[0], a: if has_alpha { src[3] } else { 255 } };
        }
        Some(buf)
    }
}
