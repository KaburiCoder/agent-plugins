use anyhow::{Context, Result};
use image::{RgbaImage, imageops};
use xcap::Monitor;

use crate::geom::ScreenRect;

pub struct MonitorInfo {
    pub rect: ScreenRect,
    pub scale: f32,
    pub primary: bool,
    pub name: String,
}

/// Monitors in the order Windows lists them; the index is what tools take as `monitor`.
pub fn monitors() -> Result<Vec<MonitorInfo>> {
    let mut out = Vec::new();
    for m in Monitor::all().context("모니터 목록")? {
        out.push(MonitorInfo {
            rect: ScreenRect::new(m.x()?, m.y()?, m.width()? as i32, m.height()? as i32),
            scale: m.scale_factor().unwrap_or(1.0),
            primary: m.is_primary().unwrap_or(false),
            name: m.friendly_name().or_else(|_| m.name()).unwrap_or_default(),
        });
    }
    Ok(out)
}

/// All monitors together.
pub fn virtual_screen() -> Result<ScreenRect> {
    monitors()?.iter().map(|m| m.rect).reduce(|a, b| a.union(&b)).context("모니터 없음")
}

/// The window's own picture, as it draws itself even where other windows cover it, and where
/// that picture is on screen (its whole window rectangle). None when it is minimized or will
/// not draw.
pub fn capture_window(h: isize) -> Option<(RgbaImage, ScreenRect)> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleBitmap, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
        DeleteObject, GetDIBits, GetWindowDC, ReleaseDC, SelectObject,
    };
    use windows::Win32::Storage::Xps::{PRINT_WINDOW_FLAGS, PrintWindow};
    /// Also what DirectComposition draws (PW_RENDERFULLCONTENT).
    const FULL_CONTENT: PRINT_WINDOW_FLAGS = PRINT_WINDOW_FLAGS(2);
    if crate::win32::is_minimized(h) {
        return None;
    }
    let r = crate::win32::rect(h)?;
    let (w, ht) = (r.width(), r.height());
    let hwnd = HWND(h as *mut _);
    let mut buf = vec![0u8; (w * ht * 4) as usize];
    let drawn = unsafe {
        let wdc = GetWindowDC(Some(hwnd));
        if wdc.is_invalid() {
            return None;
        }
        let mdc = CreateCompatibleDC(Some(wdc));
        let bmp = CreateCompatibleBitmap(wdc, w, ht);
        let old = SelectObject(mdc, bmp.into());
        let printed = PrintWindow(hwnd, mdc, FULL_CONTENT).as_bool();
        SelectObject(mdc, old);
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -ht,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let rows = GetDIBits(mdc, bmp, 0, ht as u32, Some(buf.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mdc);
        ReleaseDC(Some(hwnd), wdc);
        printed && rows == ht
    };
    if !drawn {
        return None;
    }
    for px in buf.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
        px[3] = 255;
    }
    Some((RgbaImage::from_raw(w as u32, ht as u32, buf)?, r))
}

/// Captures the monitors overlapping `rect` and returns exactly that rectangle (parts off
/// every monitor stay black).
pub fn capture_rect(rect: ScreenRect) -> Result<RgbaImage> {
    anyhow::ensure!(!rect.is_empty(), "빈 영역");
    let mut out = RgbaImage::new(rect.width() as u32, rect.height() as u32);
    let mut any = false;
    for m in Monitor::all().context("모니터 목록")? {
        let mr = ScreenRect::new(m.x()?, m.y()?, m.width()? as i32, m.height()? as i32);
        let inter = rect.intersect(&mr);
        if inter.is_empty() {
            continue;
        }
        let part = m.capture_region(
            (inter.left - mr.left) as u32,
            (inter.top - mr.top) as u32,
            inter.width() as u32,
            inter.height() as u32,
        )?;
        imageops::replace(&mut out, &part, (inter.left - rect.left) as i64, (inter.top - rect.top) as i64);
        any = true;
    }
    anyhow::ensure!(any, "화면 밖 영역");
    Ok(out)
}

/// One colour all over: what a program that draws nothing for PrintWindow leaves.
pub fn is_blank(img: &RgbaImage) -> bool {
    let Some(first) = img.pixels().next() else { return true };
    // Sampling is enough: a real window has borders, text or at least a title bar.
    img.pixels().step_by(97).all(|p| p == first)
}
