mod media;
mod config;
mod gui;

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("This application is only supported on Windows.");
        std::process::exit(1);
    }

    // Build config on startup
    let mut priority_list: Vec<(String, i32)> = Vec::new();
    priority_list.push(("spotify".to_string(), 40));
    priority_list.push(("vlc".to_string(), 35));
    priority_list.push(("music".to_string(), 30));
    priority_list.push(("chrome".to_string(), 25));
    priority_list.push(("msedge".to_string(), 25));
    let (position, size) = gui::get_taskbar_overlay_placement()
        .or_else(gui::get_taskbar_position_and_size)
        .unwrap_or(([0.0f32, 0.0f32], [420.0f32, 48.0f32]));
    let config = config::TaskbarMediaConfig::new(priority_list, size, position);
    config::init_config(config).expect("Failed to initialize app config");

    // Listen for media events in the background while the GUI is running.
    tokio::spawn(async {
        if let Err(error) = media::media_controller().await {
            eprintln!("Media controller error: {error}");
        }
    });

    gui::start_gui();
}