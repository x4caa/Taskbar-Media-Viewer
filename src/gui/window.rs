use crate::config::get_config;
use crate::gui::gui_placement;
use crate::gui::gui_util;
use crate::gui::monitor;
use crate::media;
use eframe::egui::{self, ViewportCommand};
use std::time::Duration;

const REPAINT_ACTIVE_INTERVAL: Duration = Duration::from_millis(16);
const VIEWPORT_LERP_SPEED: f32 = 14.0;
const VIEWPORT_SNAP_EPSILON: f32 = 0.25;

struct TaskbarGui {
    current_media: media::MediaInfo,
    viewport_size: [f32; 2],
    viewport_position: [f32; 2],
    mouse_passthrough_enabled: bool,
    first_frame: bool,
}

impl Default for TaskbarGui {
    fn default() -> Self {
        let (viewport_position, viewport_size) = get_config()
            .map(|config| (config.position, config.size))
            .unwrap_or(([0.0, 0.0], [gui_placement::TASKBAR_WIDGET_WIDTH, 48.0]));

        Self {
            current_media: media::MediaInfo::empty(),
            viewport_size,
            viewport_position,
            mouse_passthrough_enabled: false,
            first_frame: true,
        }
    }
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
            .with_taskbar(false)
            .with_always_on_top()
            .with_inner_size(config.size)
            .with_position(config.position),

        ..Default::default()
    };

    let _ = eframe::run_native(
        "Taskbar Media Info",
        options,
        Box::new(|cc| {
            // Frames are only drawn when something wakes the GUI, so media changes and the desktop monitor do that.
            let ctx = cc.egui_ctx.clone();
            media::set_update_notifier(move || ctx.request_repaint());
            monitor::start(cc.egui_ctx.clone());
            Ok(Box::<TaskbarGui>::default())
        }),
    );
}

impl eframe::App for TaskbarGui {
    // Keep the OS compositor-visible background fully transparent
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let overlay = monitor::overlay_state();
        let (target_position, size) = overlay.placement;

        // eframe runs the first frame before the window is ready and drops its viewport commands, so run a second one
        // to apply them, since nothing else may wake the GUI for a while.
        if std::mem::take(&mut self.first_frame) {
            ctx.request_repaint();
            gui_util::keep_overlay_out_of_task_switchers();
        } else if self.mouse_passthrough_enabled != overlay.mouse_passthrough {
            ctx.send_viewport_cmd(ViewportCommand::MousePassthrough(overlay.mouse_passthrough));
            self.mouse_passthrough_enabled = overlay.mouse_passthrough;
        }

        if (self.viewport_size[0] - size[0]).abs() > 0.5 || (self.viewport_size[1] - size[1]).abs() > 0.5 {
            self.viewport_size = size;
            ctx.send_viewport_cmd(ViewportCommand::InnerSize(egui::vec2(size[0], size[1])));
        }

        // Smoothly animate the viewport toward the latest taskbar position.
        let dt = ctx.input(|i| i.stable_dt).max(1.0 / 240.0);
        let alpha = 1.0 - (-VIEWPORT_LERP_SPEED * dt).exp();

        let mut next_position = [
            lerp(self.viewport_position[0], target_position[0], alpha),
            lerp(self.viewport_position[1], target_position[1], alpha),
        ];

        if (next_position[0] - target_position[0]).abs() <= VIEWPORT_SNAP_EPSILON {
            next_position[0] = target_position[0];
        }
        if (next_position[1] - target_position[1]).abs() <= VIEWPORT_SNAP_EPSILON {
            next_position[1] = target_position[1];
        }

        if (next_position[0] - self.viewport_position[0]).abs() > 0.01
            || (next_position[1] - self.viewport_position[1]).abs() > 0.01
        {
            self.viewport_position = next_position;
            ctx.send_viewport_cmd(ViewportCommand::OuterPosition(egui::pos2(
                self.viewport_position[0],
                self.viewport_position[1],
            )));
            ctx.request_repaint_after(REPAINT_ACTIVE_INTERVAL);
        }

        if overlay.fullscreen {
            monitor::set_interactive_rects(Vec::new(), ctx.pixels_per_point());
            return;
        }

        // Refresh media details only when the media controller reports a relevant change.
        if media::take_media_update_pending() {
            self.current_media = media::get_cached_prioritized_media();
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
                            let was_playing = self.current_media.state == "Playing";
                            let result = if was_playing {
                                media::pause_session(&self.current_media.app_id)
                            } else {
                                media::play_session(&self.current_media.app_id)
                            };

                            match result {
                                Ok(true) => {
                                    self.current_media.state = if was_playing {
                                        "Paused".to_string()
                                    } else {
                                        "Playing".to_string()
                                    };
                                    // Ensure the icon updates immediately on this click.
                                    ctx.request_repaint();
                                }
                                Ok(false) => {
                                    eprintln!("Playback state change not supported by this session.");
                                }
                                Err(error) => {
                                    eprintln!("Failed to change playback state: {error}");
                                }
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
                        let colour = if overlay.dark_mode {
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
        monitor::set_interactive_rects(rects, ctx.pixels_per_point());
    }
}
