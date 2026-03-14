mod media;
mod config;
mod taskbar_gui;

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
    let taskbar_position_and_size = taskbar_gui::get_taskbar_position_and_size().unwrap_or(([0.0f32, 0.0f32], [0.0f32, 0.0f32]));
    let position = [0.0, taskbar_position_and_size.0[1]];
    let size = [taskbar_position_and_size.1[0] * 0.25, taskbar_position_and_size.1[1]];
    let config = config::TaskbarMediaConfig::new(priority_list, size, position);
    config::init_config(config).expect("Failed to initialize app config");

    // Listen for media events in the background while the GUI is running.
    tokio::spawn(async {
        if let Err(error) = media::media_controller().await {
            eprintln!("Media controller error: {error}");
        }
    });

    taskbar_gui::start_gui();
}