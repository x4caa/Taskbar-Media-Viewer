#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod media;
mod power;
mod config;
mod fallback;
mod service_recovery;
mod gui;
mod updater;

fn main() {
    #[cfg(not(target_os = "windows"))]
    {
        eprintln!("This application is only supported on Windows.");
        std::process::exit(1);
    }
    
    let args: Vec<String> = std::env::args().collect();
    println!("Starting with args: {:?}", args);

    if args.contains(&"--update-only".to_string()) {
        if let Err(error) = updater::update_app() {
            eprintln!("Update check failed: {error}");
        }
        return;
    }

    power::enable_efficiency_mode();
    updater::remove_replaced_executables();
    let skip_update = args.contains(&"--no-update".to_string());
    if !skip_update && (updater::should_check_for_updates() || args.contains(&"--update".to_string())) {
        updater::spawn_update_check();
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

    // Listen for media events in the background while the GUI is running. A single-threaded runtime is plenty for
    // one listener, where the default runtime would start a worker thread per CPU core.
    std::thread::spawn(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .expect("Failed to start the media runtime");
        runtime.block_on(media::run_media_controller());
    });
    // Detect media another way whenever the Windows media session service is unavailable.
    fallback::start_fallback_thread();

    gui::start_gui();
}