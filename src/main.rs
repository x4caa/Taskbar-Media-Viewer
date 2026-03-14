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
    let size = [800.0, 46.0]; // TODO: Dynamically calculate this based on screen size and taskbar position
    let position = [0.0, 1033.0]; // TODO: Dynamically calculate this based on screen size and taskbar position
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