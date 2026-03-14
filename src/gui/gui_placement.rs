use std::mem::size_of;
use windows::core::{PCWSTR, w};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow
};
use windows::Win32::Foundation::{RECT as WinRECT};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation, TreeScope_Descendants};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowRect
};

// Default width target for the overlay before it is clamped to the available taskbar gap.
pub const TASKBAR_WIDGET_WIDTH: f32 = 420.0;
// Keeps a little breathing room between the overlay and nearby taskbar icons.
pub const TASKBAR_GAP_MARGIN: f32 = 20.0;

#[derive(Debug, Clone, Copy)]
pub struct TaskbarGap {
    pub x: i32,
    pub width: i32,
    pub y: i32,
    pub height: i32,
}

// Gets the taskbar bounds, falling back to the monitor work area delta when the window rect is unavailable.
fn get_taskbar_rect() -> Option<WinRECT> {
    unsafe {
        let taskbar_hwnd = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;

        let mut taskbar_rect = WinRECT::default();
        if GetWindowRect(taskbar_hwnd, &mut taskbar_rect).is_ok() {
            let width = taskbar_rect.right - taskbar_rect.left;
            let height = taskbar_rect.bottom - taskbar_rect.top;
            if width > 0 && height > 0 {
                return Some(taskbar_rect);
            }
        }

        let monitor = MonitorFromWindow(taskbar_hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }

        let mut monitor_info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(monitor, &mut monitor_info).as_bool() {
            return None;
        }

        let monitor_rect = monitor_info.rcMonitor;
        let work_rect = monitor_info.rcWork;

        let left = (work_rect.left - monitor_rect.left).max(0) as f32;
        let top = (work_rect.top - monitor_rect.top).max(0) as f32;
        let right = (monitor_rect.right - work_rect.right).max(0) as f32;
        let bottom = (monitor_rect.bottom - work_rect.bottom).max(0) as f32;

        if bottom > 0.0 {
            return Some(WinRECT {
                left: monitor_rect.left,
                top: work_rect.bottom,
                right: monitor_rect.right,
                bottom: monitor_rect.bottom,
            });
        }
        if top > 0.0 {
            return Some(WinRECT {
                left: monitor_rect.left,
                top: monitor_rect.top,
                right: monitor_rect.right,
                bottom: work_rect.top,
            });
        }
        if left > 0.0 {
            return Some(WinRECT {
                left: monitor_rect.left,
                top: monitor_rect.top,
                right: work_rect.left,
                bottom: monitor_rect.bottom,
            });
        }
        if right > 0.0 {
            return Some(WinRECT {
                left: work_rect.right,
                top: monitor_rect.top,
                right: monitor_rect.right,
                bottom: monitor_rect.bottom,
            });
        }

        None
    }
}

// Walks the taskbar accessibility tree and finds the largest horizontal gap not occupied by visible taskbar elements.
pub fn find_largest_taskbar_gap() -> Option<TaskbarGap> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        let taskbar_hwnd = FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok()?;
        let taskbar_rect = get_taskbar_rect()?;
        let taskbar_width = taskbar_rect.right - taskbar_rect.left;
        let taskbar_height = taskbar_rect.bottom - taskbar_rect.top;

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok()?;
        let taskbar_element = automation.ElementFromHandle(taskbar_hwnd).ok()?;
        let condition = automation.CreateTrueCondition().ok()?;
        let children = taskbar_element
            .FindAll(TreeScope_Descendants, &condition)
            .ok()?;
        let count = children.Length().ok()?;

        let mut intervals: Vec<(i32, i32)> = Vec::new();

        for index in 0..count {
            let Ok(child) = children.GetElement(index) else {
                continue;
            };

            if let Ok(is_offscreen) = child.CurrentIsOffscreen() {
                if is_offscreen.as_bool() {
                    continue;
                }
            }

            let Ok(rect) = child.CurrentBoundingRectangle() else {
                continue;
            };

            let left = rect.left as i32;
            let right = rect.right as i32;
            let top = rect.top as i32;
            let bottom = rect.bottom as i32;
            let width = right - left;
            let height = bottom - top;

            if width <= 0 || height <= 0 {
                continue;
            }

            if width >= (taskbar_width - 50) {
                continue;
            }

            let clamped_left = left.max(taskbar_rect.left);
            let clamped_right = right.min(taskbar_rect.right);
            let clamped_top = top.max(taskbar_rect.top);
            let clamped_bottom = bottom.min(taskbar_rect.bottom);

            if clamped_left >= clamped_right || clamped_top >= clamped_bottom {
                continue;
            }

            if (clamped_bottom - clamped_top) < (taskbar_height / 3) {
                continue;
            }

            intervals.push((clamped_left, clamped_right));
        }

        // If UI Automation yields nothing useful, treat the whole taskbar as available space.
        if intervals.is_empty() {
            return Some(TaskbarGap {
                x: taskbar_rect.left,
                width: taskbar_width,
                y: taskbar_rect.top,
                height: taskbar_height,
            });
        }

        intervals.sort_by_key(|interval| interval.0);
        let mut merged = vec![intervals[0]];

        // Merge all overlapping taskbar element bounds into continuous blocked regions.
        for current in intervals.into_iter().skip(1) {
            let last = merged.last_mut().expect("merged always contains one interval");
            if current.0 <= last.1 {
                last.1 = last.1.max(current.1);
            } else {
                merged.push(current);
            }
        }

        let mut largest_gap = TaskbarGap {
            x: taskbar_rect.left,
            width: 0,
            y: taskbar_rect.top,
            height: taskbar_height,
        };

        // Check the free space before, between, and after the blocked regions.
        let first_gap = merged[0].0 - taskbar_rect.left;
        if first_gap > largest_gap.width {
            largest_gap.width = first_gap;
            largest_gap.x = taskbar_rect.left;
        }

        for window in merged.windows(2) {
            let gap_x = window[0].1;
            let gap_width = window[1].0 - gap_x;
            if gap_width > largest_gap.width {
                largest_gap.width = gap_width;
                largest_gap.x = gap_x;
            }
        }

        let last_right = merged.last().expect("merged is never empty").1;
        let last_gap = taskbar_rect.right - last_right;
        if last_gap > largest_gap.width {
            largest_gap.width = last_gap;
            largest_gap.x = last_right;
        }

        Some(largest_gap)
    }
}

// Converts the detected taskbar gap into a centered overlay placement and clamps the width to the available space.
pub fn get_taskbar_overlay_placement() -> Option<([f32; 2], [f32; 2])> {
    let gap = find_largest_taskbar_gap()?;
    let available_width = (gap.width as f32 - TASKBAR_GAP_MARGIN).max(0.0);
    let widget_width = TASKBAR_WIDGET_WIDTH.max(available_width);
    let widget_height = gap.height as f32;
    let position_x = gap.x as f32 + ((gap.width as f32 - widget_width) / 2.0);

    Some(([position_x, gap.y as f32], [widget_width, widget_height]))
}

// Exposes the raw taskbar bounds as a fallback placement when no UI Automation gap can be found.
pub fn get_taskbar_position_and_size() -> Option<([f32; 2], [f32; 2])> {
    let rect = get_taskbar_rect()?;
    Some((
        [rect.left as f32, rect.top as f32],
        [(rect.right - rect.left) as f32, (rect.bottom - rect.top) as f32],
    ))
}