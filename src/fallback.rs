// Fallback media detection for when the Windows media session service is unavailable.
// Uses audio sessions to find which apps are making sound, then reads track info from window and browser tab titles.
use crate::media::{self, MediaInfo};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};
use windows::core::{BOOL, Interface, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::Endpoints::IAudioMeterInformation;
use windows::Win32::Media::Audio::{
    AudioSessionStateActive, DEVICE_STATE_ACTIVE, IAudioSessionControl2, IAudioSessionManager2,
    IMMDeviceEnumerator, MMDeviceEnumerator, eRender,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, TreeScope_Descendants, UIA_ControlTypePropertyId,
    UIA_TabItemControlTypeId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsWindowVisible,
    PostMessageW, WM_APPCOMMAND,
};

pub const FALLBACK_APP_PREFIX: &str = "fallback:";

const POLL_INTERVAL: Duration = Duration::from_secs(1);
// Keep reporting playback through short silences such as quiet passages or pauses in speech.
const SILENCE_GRACE: Duration = Duration::from_secs(4);
const AUDIBLE_PEAK_THRESHOLD: f32 = 0.0001;
const CHROMIUM_BROWSERS: [&str; 5] = ["msedge.exe", "chrome.exe", "brave.exe", "opera.exe", "vivaldi.exe"];
// Chromium appends this to the accessible name of tabs that are currently making sound.
const TAB_AUDIO_MARKER: &str = " - Audio playing";

#[derive(Clone, Copy)]
pub enum MediaCommand {
    NextTrack = 11,
    PreviousTrack = 12,
    PlayPause = 14,
}

struct AppWindow {
    hwnd: HWND,
    exe: String,
    title: String,
}

fn app_id_for(exe: &str) -> String {
    format!("{FALLBACK_APP_PREFIX}{exe}")
}

pub fn is_fallback_app(app_id: &str) -> bool {
    app_id.starts_with(FALLBACK_APP_PREFIX)
}

// Polls for playing media on a dedicated thread whenever the media session service is unavailable.
pub fn start_fallback_thread() {
    std::thread::spawn(|| {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let automation: Option<IUIAutomation> =
            unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER).ok() };
        let mut last_media: Option<MediaInfo> = None;
        let mut last_audible = Instant::now() - SILENCE_GRACE;

        loop {
            std::thread::sleep(POLL_INTERVAL);
            if media::session_service_healthy() {
                last_media = None;
                continue;
            }

            let windows = list_app_windows();
            let info = match detect_playing_media(&windows, automation.as_ref()) {
                Some(info) => {
                    last_audible = Instant::now();
                    last_media = Some(info.clone());
                    info
                }
                None => match &last_media {
                    // Forget remembered media once its app has closed.
                    Some(last) if !windows.iter().any(|w| app_id_for(&w.exe) == last.app_id) => {
                        last_media = None;
                        MediaInfo::empty()
                    }
                    Some(last) if last_audible.elapsed() < SILENCE_GRACE => last.clone(),
                    Some(last) => MediaInfo::new(
                        last.app_id.clone(),
                        last.title.clone(),
                        last.artist.clone(),
                        "Paused".to_string(),
                    ),
                    None => MediaInfo::empty(),
                },
            };

            media::store_fallback_media(info);
        }
    });
}

fn detect_playing_media(windows: &[AppWindow], automation: Option<&IUIAutomation>) -> Option<MediaInfo> {
    let audible = audible_apps().unwrap_or_default();
    let mut candidates: Vec<MediaInfo> = Vec::new();

    for exe in &audible {
        if CHROMIUM_BROWSERS.contains(&exe.as_str()) {
            // Browser audio comes from a background process, so find the tab that is playing instead of the window.
            let Some(automation) = automation else {
                continue;
            };
            for window in windows.iter().filter(|w| &w.exe == exe) {
                for tab_name in playing_tab_names(automation, window.hwnd) {
                    let (title, artist) = parse_tab_name(&tab_name, exe);
                    candidates.push(MediaInfo::new(app_id_for(exe), title, artist, "Playing".to_string()));
                }
            }
        } else if let Some(window) = windows.iter().find(|w| &w.exe == exe) {
            if let Some((title, artist)) = parse_window_title(exe, &window.title) {
                candidates.push(MediaInfo::new(app_id_for(exe), title, artist, "Playing".to_string()));
            }
        }
    }

    // Spotify shows the track in its title whenever it is playing, even through silent sections.
    for window in windows.iter().filter(|w| w.exe == "spotify.exe") {
        if let Some((title, artist)) = parse_window_title(&window.exe, &window.title) {
            candidates.push(MediaInfo::new(app_id_for(&window.exe), title, artist, "Playing".to_string()));
        }
    }

    candidates.into_iter().max_by_key(|m| m.score)
}

// Returns the executable names of processes currently producing sound on any output device.
fn audible_apps() -> windows::core::Result<HashSet<String>> {
    let mut audible = HashSet::new();

    unsafe {
        let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let devices = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;

        for device_index in 0..devices.GetCount()? {
            let Ok(device) = devices.Item(device_index) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let sessions = manager.GetSessionEnumerator()?;

            for session_index in 0..sessions.GetCount()? {
                let Ok(control) = sessions.GetSession(session_index) else {
                    continue;
                };
                if control.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }

                let peak = control
                    .cast::<IAudioMeterInformation>()
                    .and_then(|meter| meter.GetPeakValue())
                    .unwrap_or(0.0);
                if peak <= AUDIBLE_PEAK_THRESHOLD {
                    continue;
                }

                let pid = control
                    .cast::<IAudioSessionControl2>()
                    .and_then(|control| control.GetProcessId())
                    .unwrap_or(0);
                if let Some(exe) = process_exe_name(pid) {
                    audible.insert(exe);
                }
            }
        }
    }

    Ok(audible)
}

fn process_exe_name(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }

    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 1024];
        let mut length = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buffer.as_mut_ptr()), &mut length);
        let _ = CloseHandle(handle);
        result.ok()?;

        let path = String::from_utf16_lossy(&buffer[..length as usize]);
        path.rsplit('\\').next().map(|name| name.to_ascii_lowercase())
    }
}

// Lists visible top-level windows that have a title, along with their owning executable.
fn list_app_windows() -> Vec<AppWindow> {
    unsafe extern "system" fn collect_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let handles = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        if unsafe { IsWindowVisible(hwnd) }.as_bool() && unsafe { GetWindowTextLengthW(hwnd) } > 0 {
            handles.push(hwnd);
        }
        true.into()
    }

    let mut handles: Vec<HWND> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect_window), LPARAM(&mut handles as *mut Vec<HWND> as isize));
    }

    let mut exe_names: HashMap<u32, Option<String>> = HashMap::new();
    handles
        .into_iter()
        .filter_map(|hwnd| {
            let mut pid = 0u32;
            let mut buffer = [0u16; 512];
            let title = unsafe {
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                let length = GetWindowTextW(hwnd, &mut buffer);
                String::from_utf16_lossy(&buffer[..length.max(0) as usize])
            };
            let exe = exe_names.entry(pid).or_insert_with(|| process_exe_name(pid)).clone()?;
            Some(AppWindow { hwnd, exe, title })
        })
        .collect()
}

// Returns the accessible names of tabs in a Chromium browser window that are currently playing audio.
fn playing_tab_names(automation: &IUIAutomation, hwnd: HWND) -> Vec<String> {
    let mut names = Vec::new();

    unsafe {
        let Ok(root) = automation.ElementFromHandle(hwnd) else {
            return names;
        };
        let Ok(condition) = automation.CreatePropertyCondition(
            UIA_ControlTypePropertyId,
            &VARIANT::from(UIA_TabItemControlTypeId.0),
        ) else {
            return names;
        };
        let Ok(tabs) = root.FindAll(TreeScope_Descendants, &condition) else {
            return names;
        };

        for index in 0..tabs.Length().unwrap_or(0) {
            if let Ok(name) = tabs.GetElement(index).and_then(|tab| tab.CurrentName()) {
                let name = name.to_string();
                if name.contains(TAB_AUDIO_MARKER) {
                    names.push(name);
                }
            }
        }
    }

    names
}

fn app_display_name(exe: &str) -> String {
    match exe {
        "msedge.exe" => "Microsoft Edge".to_string(),
        "chrome.exe" => "Google Chrome".to_string(),
        _ => exe.trim_end_matches(".exe").to_string(),
    }
}

// Turns a tab name like "(3) Video Title - YouTube - Audio playing - Memory usage - 200 MB" into ("Video Title", "YouTube").
fn parse_tab_name(name: &str, exe: &str) -> (String, String) {
    let name = name.split(TAB_AUDIO_MARKER).next().unwrap_or(name);
    let name = strip_notification_count(name);

    match name.rsplit_once(" - ") {
        Some((title, site)) if !title.is_empty() => (title.to_string(), site.to_string()),
        _ => (name.to_string(), app_display_name(exe)),
    }
}

// Removes the "(3) " unread count prefix that sites like YouTube add to their titles.
fn strip_notification_count(title: &str) -> &str {
    if let Some(rest) = title.strip_prefix('(') {
        if let Some((count, remainder)) = rest.split_once(") ") {
            if !count.is_empty() && count.chars().all(|c| c.is_ascii_digit()) {
                return remainder;
            }
        }
    }
    title
}

// Returns (title, artist) for an app window title, or None when the title shows nothing is playing.
fn parse_window_title(exe: &str, title: &str) -> Option<(String, String)> {
    match exe {
        // Spotify shows "Artist - Track" while playing and just "Spotify ..." when paused.
        "spotify.exe" => {
            if title.starts_with("Spotify") {
                return None;
            }
            let (artist, track) = title.split_once(" - ")?;
            Some((track.to_string(), artist.to_string()))
        }
        "vlc.exe" => Some((
            title.trim_end_matches(" - VLC media player").to_string(),
            "VLC".to_string(),
        )),
        _ => Some((title.to_string(), app_display_name(exe))),
    }
}

// Sends a media command straight to the app's window, bypassing the media session service.
pub fn send_media_command(app_id: &str, command: MediaCommand) -> windows::core::Result<bool> {
    let Some(exe) = app_id.strip_prefix(FALLBACK_APP_PREFIX) else {
        return Ok(false);
    };

    // Browsers ignore WM_APPCOMMAND and route media keys through the session service, so they can't be controlled here.
    if CHROMIUM_BROWSERS.contains(&exe) {
        return Ok(false);
    }

    let Some(window) = list_app_windows().into_iter().find(|w| w.exe == exe) else {
        return Ok(false);
    };

    unsafe {
        PostMessageW(
            Some(window.hwnd),
            WM_APPCOMMAND,
            WPARAM(window.hwnd.0 as usize),
            LPARAM((command as isize) << 16),
        )?;
    }

    Ok(true)
}
