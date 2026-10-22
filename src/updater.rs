use self_update::cargo_crate_version;
use std::os::windows::process::CommandExt;
use std::process::Command;
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

// Runs the update check in a short-lived child process, so its HTTP and TLS stack never loads into the long-running overlay.
pub fn spawn_update_check() {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let result = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .arg("--update-only")
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
    });
    if let Err(error) = result {
        eprintln!("Failed to start update check: {error}");
    }
}

// An update renames the running exe aside, and it can only be deleted once the overlay stops using it.
// Removes those leftovers from earlier updates, which are free by the time the new version starts.
pub fn remove_replaced_executables() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let (Some(directory), Some(stem)) = (exe.parent(), exe.file_stem().and_then(|stem| stem.to_str())) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };

    let prefix = format!(".{stem}.");
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(&prefix) && name.ends_with(".__relocated__.exe") {
            let _ = std::fs::remove_file(entry.path());
        }
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
