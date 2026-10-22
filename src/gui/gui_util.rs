use std::mem::size_of;
use std::sync::atomic::{AtomicIsize, Ordering};
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT as WinRECT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, FindWindowW, GW_HWNDPREV, GetForegroundWindow, GWL_EXSTYLE, GetWindow, GetWindowLongW,
    GetWindowRect, GetWindowThreadProcessId, IsWindow,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    STYLESTRUCT, SetWindowLongW, SetWindowPos, WM_STYLECHANGING, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);

// Finds this process's overlay window once and caches it, so frequent checks don't search every window by title.
fn overlay_hwnd() -> Option<HWND> {
    let cached = HWND(OVERLAY_HWND.load(Ordering::Acquire) as *mut _);
    if !cached.0.is_null() && unsafe { IsWindow(Some(cached)) }.as_bool() {
        return Some(cached);
    }

    let own_pid = unsafe { GetCurrentProcessId() };
    let mut previous: Option<HWND> = None;
    loop {
        let hwnd = unsafe { FindWindowExW(None, previous, PCWSTR::null(), w!("Taskbar Media Info")) }.ok()?;
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid == own_pid {
            OVERLAY_HWND.store(hwnd.0 as isize, Ordering::Release);
            return Some(hwnd);
        }
        previous = Some(hwnd);
    }
}

// Returns the overlay's bounds in physical screen pixels.
pub fn overlay_window_rect() -> Option<WinRECT> {
    let hwnd = overlay_hwnd()?;
    let mut rect = WinRECT::default();
    unsafe { GetWindowRect(hwnd, &mut rect) }.ok()?;
    Some(rect)
}

// Returns true if the taskbar sits above the overlay in the z-order, which only takes a walk over the few windows above it.
pub fn overlay_below_taskbar() -> bool {
    let Some(overlay) = overlay_hwnd() else {
        return false;
    };
    let Ok(taskbar) = (unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()) }) else {
        return false;
    };

    let mut current = overlay;
    while let Ok(above) = unsafe { GetWindow(current, GW_HWNDPREV) } {
        if above == taskbar {
            return true;
        }
        current = above;
    }
    false
}

// Pins an overlay window to the topmost z-order to ensure it stays above the taskbar and other windows
pub fn pin_overlay_topmost() {
    if let Some(hwnd) = overlay_hwnd() {
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

fn with_tool_window_style(ex_style: u32) -> u32 {
    (ex_style | WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) & !WS_EX_APPWINDOW.0
}

// Keeps the overlay a tool window for good, so it stays out of the taskbar and Alt-Tab. winit rewrites the whole
// extended style whenever it changes a window flag such as mouse passthrough, so the style is corrected while it is
// being changed. Must be called on the thread that owns the window.
pub fn keep_overlay_out_of_task_switchers() {
    let Some(hwnd) = overlay_hwnd() else {
        return;
    };
    if unsafe { SetWindowSubclass(hwnd, Some(enforce_tool_window_style), 1, 0) }.as_bool() {
        hide_overlay_from_task_switchers();
    }
}

unsafe extern "system" fn enforce_tool_window_style(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if msg == WM_STYLECHANGING && wparam.0 as i32 == GWL_EXSTYLE.0 {
        let style = unsafe { &mut *(lparam.0 as *mut STYLESTRUCT) };
        style.styleNew = with_tool_window_style(style.styleNew);
    }
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

// Convert the window to a Windows tool window so it is hidden from taskbar and Alt-Tab.
fn hide_overlay_from_task_switchers() {
    if let Some(hwnd) = overlay_hwnd() {
        let ex_style = unsafe { GetWindowLongW(hwnd, GWL_EXSTYLE) };
        let new_style = with_tool_window_style(ex_style as u32);

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
    if let Some(overlay_hwnd) = overlay_hwnd() {
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