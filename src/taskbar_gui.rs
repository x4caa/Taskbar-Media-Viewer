use crate::media;
use eframe::egui;
use std::time::Duration;
#[cfg(target_os = "windows")]
use windows::core::PCWSTR;
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_EXSTYLE, HWND_TOPMOST,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, WS_EX_APPWINDOW,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
};

pub struct TaskbarGui {
    current_media: media::MediaInfo,
}

impl Default for TaskbarGui {
    fn default() -> Self {
        Self {
            current_media: media::MediaInfo::new(
                String::new(),
                "No Active Media Session".to_string(),
                String::new(),
            ),
        }
    }
}

// Pins an overlay window to the topmost z-order to ensure it stays above the taskbar and other windows
#[cfg(target_os = "windows")]
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
#[cfg(target_os = "windows")]
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

impl eframe::App for TaskbarGui {
    // Keep the OS compositor-visible background fully transparent
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(50));
        #[cfg(target_os = "windows")]
        hide_overlay_from_task_switchers();
        #[cfg(target_os = "windows")]
        pin_overlay_topmost();

        // Get the current prioritized media session and update the displayed info
        let song = media::get_prioritized_media().unwrap_or_else(|| {
            media::MediaInfo::new(
                String::new(),
                "No Active Media Session".to_string(),
                String::new(),
            )
        });
        self.current_media = song;

        // Draw the UI
        let frame = egui::Frame::new().fill(egui::Color32::TRANSPARENT);
        egui::CentralPanel::default().frame(frame).show(ctx, |ui| {
            let title_text = egui::RichText::new(self.current_media.title.clone())
                .size(18.0)
                .color(egui::Color32::WHITE);
            let artist_text = egui::RichText::new(self.current_media.artist.clone())
                .size(14.0)
                .color(egui::Color32::WHITE);

            ui.vertical_centered(|ui| {
                ui.label(title_text);
                ui.label(artist_text);
            });
        });
    }
}
