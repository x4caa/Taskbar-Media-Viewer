use eframe::egui::{self};
use std::mem::size_of;
use windows::core::{PCWSTR};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow
};
use windows::Win32::Foundation::{POINT, RECT as WinRECT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GWL_EXSTYLE, GetCursorPos, GetWindowLongW, GetWindowRect,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowLongW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

// Returns true if the OS cursor is currently over any of the given egui rects (in logical pixels).
// Uses Win32 GetCursorPos so it works even when MousePassthrough is active.
pub fn cursor_over_rects(rects: &[egui::Rect], pixels_per_point: f32) -> bool {
    let title_utf16: Vec<u16> = "Taskbar Media Info\0".encode_utf16().collect();
    let Ok(hwnd) = (unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title_utf16.as_ptr())) }) else {
        return false;
    };
    let mut cursor = POINT::default();
    if unsafe { GetCursorPos(&mut cursor) }.is_err() {
        return false;
    }
    let mut win_rect = WinRECT::default();
    if unsafe { GetWindowRect(hwnd, &mut win_rect) }.is_err() {
        return false;
    }
    // Convert physical screen coords → window-local logical pixels
    let lx = (cursor.x - win_rect.left) as f32 / pixels_per_point;
    let ly = (cursor.y - win_rect.top) as f32 / pixels_per_point;
    let local_pos = egui::pos2(lx, ly);
    rects.iter().any(|r| r.contains(local_pos))
}

// Pins an overlay window to the topmost z-order to ensure it stays above the taskbar and other windows
pub fn pin_overlay_topmost() {
    let title_utf16: Vec<u16> = "Taskbar Media Info\0".encode_utf16().collect();

    let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title_utf16.as_ptr())) };
    if let Ok(hwnd) = hwnd {
        let _ = unsafe {
            SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }
}

// Convert the window to a Windows tool window so it is hidden from taskbar and Alt-Tab.
pub fn hide_overlay_from_task_switchers() {
    let title_utf16: Vec<u16> = "Taskbar Media Info\0".encode_utf16().collect();
    let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title_utf16.as_ptr())) };

    if let Ok(hwnd) = hwnd {
        let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
        let mut new_style = ex_style as u32;
        new_style |= WS_EX_TOOLWINDOW.0 as u32;
        new_style |= WS_EX_NOACTIVATE.0 as u32;
        new_style &= !(WS_EX_APPWINDOW.0 as u32);

        if new_style != ex_style as u32 {
            let _ = unsafe { SetWindowLongW(hwnd, GWL_EXSTYLE, new_style as i32) };
            let _ = unsafe {
                SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER | SWP_FRAMECHANGED,
                )
            };
        }
    }
}

// Returns true when the foreground app occupies the full monitor area.
pub fn is_foreground_fullscreen() -> bool {
    let foreground = unsafe { GetForegroundWindow() };
    if foreground.0.is_null() {
        return false;
    }

    // Ignore our own overlay window so it doesn't pause itself.
    let title_utf16: Vec<u16> = "Taskbar Media Info\0".encode_utf16().collect();
    if let Ok(overlay_hwnd) = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(title_utf16.as_ptr())) } {
        if foreground == overlay_hwnd {
            return false;
        }
    }

    let mut win_rect = WinRECT::default();
    if unsafe { GetWindowRect(foreground, &mut win_rect) }.is_err() {
        return false;
    }

    let monitor = unsafe { MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST) };
    if monitor.0.is_null() {
        return false;
    }

    let mut monitor_info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    if !unsafe { GetMonitorInfoW(monitor, &mut monitor_info) }.as_bool() {
        return false;
    }

    let monitor_rect = monitor_info.rcMonitor;
    let tolerance = 2;

    (win_rect.left - monitor_rect.left).abs() <= tolerance
        && (win_rect.top - monitor_rect.top).abs() <= tolerance
        && (win_rect.right - monitor_rect.right).abs() <= tolerance
        && (win_rect.bottom - monitor_rect.bottom).abs() <= tolerance
}

// Returns true if the user has dark mode enabled in Windows settings. Used to adjust text color for better visibility.
pub fn is_dark_mode() -> Result<bool, windows::core::Error> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let personalize = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")?;
    let apps_use_light_theme: u32 = personalize.get_value("AppsUseLightTheme")?;

    Ok(apps_use_light_theme == 0)
}