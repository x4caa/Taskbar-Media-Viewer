use eframe::egui;

mod media;
mod config;
mod taskbar_gui;

fn main() {
    // Make config with a priority list of app identifiers
    let mut priority_list: Vec<String> = Vec::new();
    priority_list.push("Spotify.exe".to_string());
    priority_list.push("chrome.exe".to_string());
    priority_list.push("MSEdge".to_string());
    let size = [800.0, 56.0];
    let position = [0.0, 1035.0]; // TODO: Dynamically calculate this based on screen size and taskbar position
    let config = config::TaskbarMediaConfig::new(priority_list, size, position);
    config::init_config(config).expect("Failed to initialize app config");

    // Start the GUI app
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_inner_size(size)
            .with_position(position),

        ..Default::default()
    };

    let _ = eframe::run_native(
        "Taskbar Media Info",
        options,
        Box::new(|_cc| Ok(Box::<taskbar_gui::TaskbarGui>::default())),
    );
}