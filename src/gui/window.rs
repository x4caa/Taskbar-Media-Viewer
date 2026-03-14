use crate::config::get_config;
use crate::media;
use eframe::egui::{self, ViewportCommand};
use std::time::{Duration, Instant};
use crate::gui::gui_placement;
use crate::gui::gui_util;

// Poll the taskbar layout often enough to follow pinned app changes without redrawing constantly.
const TASKBAR_POLL_INTERVAL: Duration = Duration::from_millis(100);
const VIEWPORT_LERP_SPEED: f32 = 14.0;
const VIEWPORT_SNAP_EPSILON: f32 = 0.25;

struct TaskbarGui {
    current_media: media::MediaInfo,
    interactive_rects: Vec<egui::Rect>,
    viewport_size: [f32; 2],
    viewport_position: [f32; 2],
    target_viewport_position: [f32; 2],
    last_taskbar_poll: Instant,
}

impl Default for TaskbarGui {
    fn default() -> Self {
        let (viewport_position, viewport_size) = get_config()
            .map(|config| (config.position, config.size))
            .unwrap_or(([0.0, 0.0], [gui_placement::TASKBAR_WIDGET_WIDTH, 48.0]));

        Self {
            current_media: media::MediaInfo::new(
                String::new(),
                "No Active Media Session".to_string(),
                String::new(),
                String::new(),
            ),
            interactive_rects: Vec::new(),
            viewport_size,
            viewport_position,
            target_viewport_position: viewport_position,
            last_taskbar_poll: Instant::now() - TASKBAR_POLL_INTERVAL,
        }
    }
}

// Ignore tiny floating point differences so we only move the viewport when the taskbar layout meaningfully changes.
fn placement_has_changed(position: [f32; 2], size: [f32; 2], new_position: [f32; 2], new_size: [f32; 2]) -> bool {
    const EPSILON: f32 = 0.5;

    (position[0] - new_position[0]).abs() > EPSILON
        || (position[1] - new_position[1]).abs() > EPSILON
        || (size[0] - new_size[0]).abs() > EPSILON
        || (size[1] - new_size[1]).abs() > EPSILON
}

fn lerp(current: f32, target: f32, alpha: f32) -> f32 {
    current + (target - current) * alpha
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
        ctx.request_repaint_after(Duration::from_millis(16));

        // Re-measure the taskbar periodically so the overlay follows centered icons and tray changes.
        if self.last_taskbar_poll.elapsed() >= TASKBAR_POLL_INTERVAL {
            self.last_taskbar_poll = Instant::now();

            if let Some((position, size)) = gui_placement::get_taskbar_overlay_placement().or_else(gui_placement::get_taskbar_position_and_size) {
                if placement_has_changed(self.target_viewport_position, self.viewport_size, position, size) {
                    self.target_viewport_position = position;

                    if (self.viewport_size[0] - size[0]).abs() > 0.5 || (self.viewport_size[1] - size[1]).abs() > 0.5 {
                        self.viewport_size = size;
                        ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
                    }
                }
            }
        }

        // Smoothly animate the viewport toward the latest taskbar position.
        let dt = ctx.input(|i| i.stable_dt).max(1.0 / 240.0);
        let alpha = 1.0 - (-VIEWPORT_LERP_SPEED * dt).exp();

        let mut next_position = [
            lerp(self.viewport_position[0], self.target_viewport_position[0], alpha),
            lerp(self.viewport_position[1], self.target_viewport_position[1], alpha),
        ];

        if (next_position[0] - self.target_viewport_position[0]).abs() <= VIEWPORT_SNAP_EPSILON {
            next_position[0] = self.target_viewport_position[0];
        }
        if (next_position[1] - self.target_viewport_position[1]).abs() <= VIEWPORT_SNAP_EPSILON {
            next_position[1] = self.target_viewport_position[1];
        }

        if (next_position[0] - self.viewport_position[0]).abs() > 0.01
            || (next_position[1] - self.viewport_position[1]).abs() > 0.01
        {
            self.viewport_position = next_position;
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(
                self.viewport_position[0],
                self.viewport_position[1],
            )));
        }

        if gui_util::is_foreground_fullscreen() {
            self.interactive_rects.clear();
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(true));
            return;
        }

        gui_util::hide_overlay_from_task_switchers();
        gui_util::pin_overlay_topmost();

        // Refresh media details only when the media controller reports a relevant change.
        if media::take_media_update_pending() {
            if let Ok(song) = media::get_prioritized_media() {
                self.current_media = song;
            }
        }

        // Draw the UI and collect interactive widget rects
        let frame = egui::Frame::new().fill(egui::Color32::TRANSPARENT);
        let size = self.viewport_size;
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
                        let colour = if gui_util::is_dark_mode().unwrap_or(false) {
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
                        ui.add_sized([ui.available_width(), button_size], |ui: &mut egui::Ui| {
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                ui.add(egui::Label::new(title_text).truncate());
                                ui.add(egui::Label::new(artist_text).truncate());
                            })
                            .response
                        });
                    });
                });
                button_rects
            })
            .inner;
        self.interactive_rects = rects;

        // Use Win32 GetCursorPos so hover detection works even when passthrough is active
        let over_interactive = gui_util::cursor_over_rects(&self.interactive_rects, ctx.pixels_per_point());

        // println!("Over interactive: {}", over_interactive);
        ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(!over_interactive));
    }
}