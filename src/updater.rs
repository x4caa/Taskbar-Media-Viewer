use self_update::cargo_crate_version;
use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

// Checks the registry for the last update check time and returns true if it's been more than 1 hour.
pub fn should_check_for_updates() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let path = "Software\\TaskbarMediaViewer";
    let (key, _) = hkcu
        .create_subkey(path)
        .expect("Failed to create or open registry key");

    let last_check: u64 = key.get_value("LastUpdateCheck").unwrap_or(0);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs();

    // Check for updates if it's been more than 1 hours since the last check
    if now - last_check > 1 * 60 * 60 {
        key.set_value("LastUpdateCheck", &now)
            .expect("Failed to update registry value");
        true
    } else {
        false
    }
}

// Performs a self-update by checking GitHub releases and downloading the latest version if available.
pub fn update_app() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    println!("Checking for updates...");

    let status = self_update::backends::github::Update::configure()
        .repo_owner("x4caa")
        .repo_name("Taskbar-Media-Viewer")
        .bin_name("taskbar_media_viewer")
        .show_download_progress(true)
        .no_confirm(true)
        .current_version(cargo_crate_version!())
        .build()?
        .update()?;

    if status.updated() {
        println!("Successfully updated to version {}!", status.version());
    } else {
        println!(
            "You are already on the latest version ({}).",
            status.version()
        );
    }

    Ok(())
}
