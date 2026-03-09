use crate::config::TaskbarMediaConfig;

mod media;
mod config;

fn main() {
    // Make config with a priority list of app identifiers
    let mut priority_list: Vec<String> = Vec::new();
    priority_list.push("Spotify.exe".to_string());
    priority_list.push("chrome.exe".to_string());
    priority_list.push("MSEdge".to_string());
    let config = TaskbarMediaConfig::new(priority_list);

    // Get the prioritized media session and print its info
    match media::get_prioritized_media(&config.priority_list) {
        Some(selected) => println!(
            "Selected by priority: {} | {} | {}",
            selected.app_id, selected.title, selected.artist
        ),
        None => println!("No active media sessions found"),
    }
}