//! The Windows taskbar overlay icon.
//!
//! Windows has no numeric badge API for a desktop app; what it has is a 16x16
//! overlay drawn on the corner of the taskbar button, and the number has to be
//! rendered into that bitmap. [`crate::icon`] does the rendering so the picture
//! is identical to the tray icon and is asserted pixel by pixel in tests that
//! run anywhere.
//!
//! `ITaskbarList3` is a COM object and the overlay belongs to one `HWND`, so
//! this backend needs the main window handle and a COM-initialised thread.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO,
};
use windows::core::{HSTRING, PCWSTR};

use crate::badge::{Badge, WindowHandle, overlay_description};
use crate::capability::{Support, Unavailable};
use crate::icon::render_count_icon;

/// Taskbar overlay icons are 16x16 at 100% scaling; Windows downsamples a
/// larger one, and a larger one is not sharper because the source is a 5x7
/// bitmap font.
const OVERLAY_SIZE: u32 = 16;

pub struct TaskbarOverlayBadge {
    taskbar: ITaskbarList3,
    hwnd: HWND,
}

// SAFETY: `ITaskbarList3` is used only from the thread that created it in
// practice, and the `Badge` trait requires `Send + Sync`. Callers are told in
// the module docs to drive the badge from the UI thread; the COM apartment
// enforces the rest.
unsafe impl Send for TaskbarOverlayBadge {}
unsafe impl Sync for TaskbarOverlayBadge {}

impl TaskbarOverlayBadge {
    pub fn connect(window: WindowHandle) -> Result<Self, Unavailable> {
        // SAFETY: initialising the calling thread's apartment. A thread already
        // in a different apartment returns RPC_E_CHANGED_MODE, which is not
        // fatal for an in-process object.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        }
        // SAFETY: `TaskbarList` is a registered in-process COM class.
        let taskbar: ITaskbarList3 =
            unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }.map_err(|e| {
                Unavailable::service_missing(format!(
                    "CoCreateInstance(TaskbarList) failed: {e}. The taskbar is unavailable in \
                     Server Core and in sessions without Explorer."
                ))
            })?;
        // SAFETY: freshly created interface pointer.
        unsafe { taskbar.HrInit() }.map_err(|e| {
            Unavailable::runtime_error(format!("ITaskbarList3::HrInit failed: {e}"))
        })?;
        Ok(Self { taskbar, hwnd: HWND(window.0 as *mut core::ffi::c_void) })
    }
}

impl Badge for TaskbarOverlayBadge {
    fn capability(&self) -> Support {
        if self.hwnd.0.is_null() {
            return Support::Missing(Unavailable::runtime_error(
                "the window handle is null; the overlay icon has no taskbar button to attach to",
            ));
        }
        Support::Available
    }

    fn set_count(&self, count: u32) -> Result<(), Unavailable> {
        let Some(image) = render_count_icon(OVERLAY_SIZE, count) else {
            // SAFETY: a null HICON is the documented way to clear the overlay.
            return unsafe { self.taskbar.SetOverlayIcon(self.hwnd, HICON::default(), PCWSTR::null()) }
                .map_err(|e| {
                    Unavailable::runtime_error(format!("SetOverlayIcon(clear) failed: {e}"))
                });
        };

        let icon = create_hicon(&image.to_bgra(), OVERLAY_SIZE)?;
        let description = HSTRING::from(overlay_description(count));
        // SAFETY: `icon` is a live HICON and `description` outlives the call.
        let result = unsafe {
            self.taskbar.SetOverlayIcon(self.hwnd, icon, PCWSTR(description.as_ptr()))
        };
        // The shell copies the icon, so it is ours to free either way.
        // SAFETY: `icon` came from CreateIconIndirect and is not used again.
        unsafe {
            let _ = DestroyIcon(icon);
        }
        result.map_err(|e| Unavailable::runtime_error(format!("SetOverlayIcon failed: {e}")))
    }
}

/// Build an `HICON` from top-down 32-bit BGRA pixels.
fn create_hicon(bgra: &[u8], size: u32) -> Result<HICON, Unavailable> {
    let expected = (size * size * 4) as usize;
    if bgra.len() != expected {
        return Err(Unavailable::runtime_error(format!(
            "icon buffer is {} bytes, expected {expected}",
            bgra.len()
        )));
    }

    let mut info = BITMAPINFO::default();
    info.bmiHeader.biSize = core::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = size as i32;
    // Negative height means top-down, matching our row order.
    info.bmiHeader.biHeight = -(size as i32);
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;

    let mut bits: *mut core::ffi::c_void = core::ptr::null_mut();
    // SAFETY: `info` describes a 32-bit DIB and `bits` receives the pixel
    // pointer the call allocates.
    let color: HBITMAP =
        unsafe { CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0) }.map_err(
            |e| Unavailable::runtime_error(format!("CreateDIBSection failed: {e}")),
        )?;
    if bits.is_null() {
        // SAFETY: `color` is a valid GDI object we own.
        unsafe {
            let _ = DeleteObject(HGDIOBJ(color.0));
        }
        return Err(Unavailable::runtime_error("CreateDIBSection returned no pixel buffer"));
    }
    // SAFETY: the DIB section is exactly `expected` bytes of top-down BGRA and
    // `bgra` was length-checked above.
    unsafe {
        core::ptr::copy_nonoverlapping(bgra.as_ptr(), bits.cast::<u8>(), expected);
    }

    // A 1bpp mask is required even though a 32-bit colour bitmap carries alpha.
    // All-zero means "show the colour bitmap everywhere".
    // SAFETY: a 1bpp monochrome bitmap of the same dimensions.
    let mask: HBITMAP = unsafe { CreateBitmap(size as i32, size as i32, 1, 1, None) };

    let icon_info = ICONINFO {
        fIcon: true.into(),
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: mask,
        hbmColor: color,
    };
    // SAFETY: both bitmaps are live for the duration of the call, which is all
    // CreateIconIndirect requires; it copies them.
    let icon = unsafe { CreateIconIndirect(&icon_info) };
    // SAFETY: CreateIconIndirect copied both bitmaps.
    unsafe {
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
    }
    icon.map_err(|e| Unavailable::runtime_error(format!("CreateIconIndirect failed: {e}")))
}
