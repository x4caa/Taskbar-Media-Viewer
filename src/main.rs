mod media;
mod config;
mod taskbar_gui;

fn main() {
    // Make config with a priority list of app identifiers
    let mut priority_list: Vec<(String, i32)> = Vec::new();
    priority_list.push(("spotify".to_string(), 40));
    priority_list.push(("vlc".to_string(), 35));
    priority_list.push(("music".to_string(), 30));
    priority_list.push(("chrome".to_string(), 25));
    priority_list.push(("msedge".to_string(), 25));
    let size = [800.0, 56.0]; // TODO: Dynamically calculate this based on screen size and taskbar position
    let position = [0.0, 1035.0]; // TODO: Dynamically calculate this based on screen size and taskbar position
    let config = config::TaskbarMediaConfig::new(priority_list, size, position);
    config::init_config(config).expect("Failed to initialize app config");

    taskbar_gui::start_gui();
}