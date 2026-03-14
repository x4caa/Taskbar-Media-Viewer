use crate::config::get_config;
use crate::media;
use eframe::egui::{self, ViewportCommand};
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;
use std::mem::size_of;
use std::time::Duration;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Foundation::{POINT, RECT as WinRECT};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetForegroundWindow, GWL_EXSTYLE, GetCursorPos, GetWindowLongW, GetWindowRect,
    HWND_TOPMOST, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SetWindowLongW, SetWindowPos, WS_EX_APPWINDOW, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};
use windows::core::PCWSTR;

struct TaskbarGui {
    current_media: media::MediaInfo,
    interactive_rects: Vec<egui::Rect>,
}

impl Default for TaskbarGui {
    fn default() -> Self {
        Self {
            current_media: media::MediaInfo::new(
                String::new(),
                "No Active Media Session".to_string(),
                String::new(),
                String::new(),
            ),
            interactive_rects: Vec::new(),
        }
    }
}

// Returns true if the OS cursor is currently over any of the given egui rects (in logical pixels).
// Uses Win32 GetCursorPos so it works even when MousePassthrough is active.
fn cursor_over_rects(rects: &[egui::Rect], pixels_per_point: f32) -> bool {
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
fn pin_overlay_topmost() {
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
fn hide_overlay_from_task_switchers() {
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
fn is_foreground_fullscreen() -> bool {
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
fn is_dark_mode() -> Result<bool, windows::core::Error> {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let personalize = hkcu.open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")?;
    let apps_use_light_theme: u32 = personalize.get_value("AppsUseLightTheme")?;

    Ok(apps_use_light_theme == 0)
}

// Start the GUI app
pub fn start_gui() {
    let config = get_config().unwrap();

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_inner_size(config.size)
            .with_position(config.position),

        ..Default::default()
    };

    let _ = eframe::run_native(
        "Taskbar Media Info",
        options,
        Box::new(|_cc| Ok(Box::<TaskbarGui>::default())),
    );
}

impl eframe::App for TaskbarGui {
    // Keep the OS compositor-visible background fully transparent
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));

        if is_foreground_fullscreen() {
            self.interactive_rects.clear();
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
            return;
        }

        hide_overlay_from_task_switchers();
        pin_overlay_topmost();

        // Refresh media details only when the media controller reports a relevant change.
        if media::take_media_update_pending() {
            if let Ok(song) = media::get_prioritized_media() {
                self.current_media = song;
            }
        }

        // Draw the UI and collect interactive widget rects
        let frame = egui::Frame::new().fill(egui::Color32::TRANSPARENT);
        let size = get_config().unwrap().size;
        let button_size = size[1] * 0.8;
        let spacer_size = size[1] * 0.1;
        let rects = egui::CentralPanel::default()
            .frame(frame)
            .show(ctx, |ui| {
                let mut button_rects = Vec::new();
                ui.vertical_centered(|ui| {
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        // Small margin on the left
                        ui.add_space(spacer_size);

                        // Previous track button
                        let r = ui.add_sized([button_size, button_size], egui::Button::new("⏮"));
                        if r.clicked() {
                            let result = media::previous_session(&self.current_media.app_id);
                            if let Err(error) = result {
                                eprintln!("Failed to skip to previous track: {error}");
                            }
                        }
                        button_rects.push(r.rect);

                        // Play/pause button
                        let play_pause_label = if self.current_media.state == "Playing" { "⏸" } else { "▶" };
                        let r = ui.add_sized([button_size, button_size], egui::Button::new(play_pause_label));
                        if r.clicked() {
                            let result = if self.current_media.state == "Playing" {
                                media::pause_session(&self.current_media.app_id)
                            } else {
                                media::play_session(&self.current_media.app_id)
                            };

                            if let Err(error) = result {
                                eprintln!("Failed to change playback state: {error}");
                            }
                        }
                        button_rects.push(r.rect);

                        // Next track button
                        let r = ui.add_sized([button_size, button_size], egui::Button::new("⏭"));
                        if r.clicked() {
                            let result = media::next_session(&self.current_media.app_id);
                            if let Err(error) = result {
                                eprintln!("Failed to skip to next track: {error}");
                            }
                        }
                        button_rects.push(r.rect);

                        // Title and artist labels
                        let colour = if is_dark_mode().unwrap_or(false) {
                            egui::Color32::WHITE
                        } else {
                            egui::Color32::BLACK
                        };
                        let title_text = egui::RichText::new(self.current_media.title.clone())
                            .size(18.0)
                            .color(colour);
                        let artist_text = egui::RichText::new(self.current_media.artist.clone())
                            .strong()
                            .size(14.0)
                            .color(colour);
                        ui.vertical(|ui| {
                            ui.add(egui::Label::new(title_text).truncate());
                            ui.add(egui::Label::new(artist_text).truncate());
                        });
                    });
                });
                button_rects
            })
            .inner;
        self.interactive_rects = rects;

        // Use Win32 GetCursorPos so hover detection works even when passthrough is active
        let over_interactive = cursor_over_rects(&self.interactive_rects, ctx.pixels_per_point());

        // println!("Over interactive: {}", over_interactive);
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(!over_interactive));
    }
}
