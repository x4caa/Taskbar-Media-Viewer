use crate::config::get_config;
use crate::fallback::{self, MediaCommand};
use crate::service_recovery;
use futures_util::StreamExt;
use nowhear::source::PlatformMediaSource;
use nowhear::{MediaSource, MediaSourceBuilder, Result, MediaEvent};
use std::sync::{Mutex, OnceLock, RwLock};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::task::JoinHandle;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
};

#[derive(Clone, PartialEq)]
#[allow(unused)]
pub struct MediaInfo {
    pub app_id: String,
    pub title: String,
    pub artist: String,
    pub state: String,
    pub score: i32,
}

impl MediaInfo {
    pub fn new(app_id: String, title: String, artist: String, state: String) -> Self {
        let score = total_priority_score(&app_id, &state, !title.is_empty());
        Self {
            app_id,
            title,
            artist,
            state,
            score,
        }
    }

    pub fn empty() -> Self {
        Self {
            app_id: String::new(),
            title: "No Active Media Session".to_string(),
            artist: String::new(),
            state: String::new(),
            score: 0,
        }
    }
}

static MEDIA_UPDATE_PENDING: AtomicBool = AtomicBool::new(true);
static UPDATE_NOTIFIER: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();
static LATEST_MEDIA: OnceLock<RwLock<MediaInfo>> = OnceLock::new();
// True while the Windows media session service is answering; the fallback detector takes over otherwise.
static SESSION_SERVICE_HEALTHY: AtomicBool = AtomicBool::new(false);
// Reused across button clicks, and dropped when it stops working because the service restarted.
static SESSION_MANAGER: Mutex<Option<GlobalSystemMediaTransportControlsSessionManager>> = Mutex::new(None);

const CONTROLLER_RETRY_DELAY: Duration = Duration::from_secs(5);
const SESSION_QUERY_TIMEOUT: Duration = Duration::from_secs(5);
// The event stream goes quiet if the service hangs, so query it periodically to notice.
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_secs(15);
// How long the service must stay unresponsive before it is restarted.
const SERVICE_HANG_THRESHOLD: Duration = Duration::from_secs(30);
// Windows only restarts the service automatically a couple of times a day, so restart it sparingly.
const SERVICE_RESTART_COOLDOWN: Duration = Duration::from_secs(10 * 60);

// Why the connection to the session service ended, so only a hung service gets restarted.
enum ControllerError {
    // The service isn't answering, which restarting it fixes.
    Unresponsive(String),
    // Anything else, such as one session failing to report its info, which only needs a reconnect.
    Failed(String),
}

fn latest_media_store() -> &'static RwLock<MediaInfo> {
    LATEST_MEDIA.get_or_init(|| RwLock::new(MediaInfo::empty()))
}

// Registers a callback that wakes the GUI whenever the media info changes.
pub fn set_update_notifier(notifier: impl Fn() + Send + Sync + 'static) {
    let _ = UPDATE_NOTIFIER.set(Box::new(notifier));
}

// Marks media info as stale and wakes the GUI so it refreshes.
pub fn mark_media_update_pending() {
    MEDIA_UPDATE_PENDING.store(true, Ordering::Release);
    if let Some(notifier) = UPDATE_NOTIFIER.get() {
        notifier();
    }
}

// Returns true when an update is pending and consumes the flag.
pub fn take_media_update_pending() -> bool {
    MEDIA_UPDATE_PENDING.swap(false, Ordering::AcqRel)
}

pub fn get_cached_prioritized_media() -> MediaInfo {
    latest_media_store()
        .read()
        .map(|media| media.clone())
        .unwrap_or_else(|_| MediaInfo::empty())
}

pub fn session_service_healthy() -> bool {
    SESSION_SERVICE_HEALTHY.load(Ordering::Acquire)
}

// Stores media found by the fallback detector, unless the session service has recovered in the meantime.
pub fn store_fallback_media(info: MediaInfo) {
    if let Ok(mut latest) = latest_media_store().write() {
        if session_service_healthy() || *latest == info {
            return;
        }
        *latest = info;
    }
    mark_media_update_pending();
}

// Gets the media session manager, which provides access to all current media sessions
fn get_session_manager() -> windows::core::Result<GlobalSystemMediaTransportControlsSessionManager>
{
    let mut cached = SESSION_MANAGER.lock().unwrap();
    if let Some(manager) = cached.as_ref() {
        return Ok(manager.clone());
    }
    let manager = pollster::block_on(GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?)?;
    *cached = Some(manager.clone());
    Ok(manager)
}

fn find_session_by_app_id(
    app_id: &str,
) -> windows::core::Result<Option<GlobalSystemMediaTransportControlsSession>> {
    match find_session_in(&get_session_manager()?, app_id) {
        // The cached manager stops working if the service restarted, so reconnect once.
        Err(_) => {
            *SESSION_MANAGER.lock().unwrap() = None;
            find_session_in(&get_session_manager()?, app_id)
        }
        result => result,
    }
}

fn find_session_in(
    manager: &GlobalSystemMediaTransportControlsSessionManager,
    app_id: &str,
) -> windows::core::Result<Option<GlobalSystemMediaTransportControlsSession>> {
    let sessions = manager.GetSessions()?;
    let size = sessions.Size()?;

    for index in 0..size {
        let session = sessions.GetAt(index)?;
        if session.SourceAppUserModelId()?.to_string() == app_id {
            return Ok(Some(session));
        }
    }

    Ok(None)
}

// Control playback for the session with the given app_id, if its playing
pub fn pause_session(app_id: &str) -> windows::core::Result<bool> {
    if fallback::is_fallback_app(app_id) {
        return fallback::send_media_command(app_id, MediaCommand::PlayPause);
    }

    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TryPauseAsync()?)
}

// Plays the session with the given app_id, if its playing
pub fn play_session(app_id: &str) -> windows::core::Result<bool> {
    if fallback::is_fallback_app(app_id) {
        return fallback::send_media_command(app_id, MediaCommand::PlayPause);
    }

    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TryPlayAsync()?)
}

// Skips to the next track in the session with the given app_id, if supported
pub fn next_session(app_id: &str) -> windows::core::Result<bool> {
    if fallback::is_fallback_app(app_id) {
        return fallback::send_media_command(app_id, MediaCommand::NextTrack);
    }

    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TrySkipNextAsync()?)
}

// Skip to the previous track in the session with the given app_id, if supported
pub fn previous_session(app_id: &str) -> windows::core::Result<bool> {
    if fallback::is_fallback_app(app_id) {
        return fallback::send_media_command(app_id, MediaCommand::PreviousTrack);
    }

    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TrySkipPreviousAsync()?)
}

// Connects to the media session service. Built once per connection, since the source is tied to the
// service process it connected to and stops working if that process is restarted.
fn build_media_source() -> Result<PlatformMediaSource> {
    pollster::block_on(MediaSourceBuilder::new().build())
}

// Get the current media sessions and their info
async fn get_current(source: &PlatformMediaSource) -> Result<Vec<MediaInfo>> {
    let players = source.list_players().await?;
    let mut media_info_list: Vec<MediaInfo> = Vec::new();

    for player in &players {
        let media = source.get_player(player).await?;
        let playback_state = format!("{:?}", media.playback_state);
        let (title, artist) = if let Some(track) = media.current_track.as_ref() {
            (track.title.clone(), track.artist.join(", "))
        } else {
            (
                "No track currently playing.".to_string(),
                "No artist available.".to_string(),
            )
        };

        media_info_list.push(MediaInfo::new(
            player.clone(),
            title,
            artist,
            playback_state,
        ));
    }

    Ok(media_info_list)
}

// Assigns priority based on playback state
fn playback_priority(playback_state: &str) -> i32 {
    match playback_state.to_ascii_lowercase().as_str() {
        "playing" => 3,
        "paused" => 2,
        "stopped" => 1,
        _ => 0,
    }
}

// Assigns priority based on app name, with preferred apps getting higher scores
fn app_priority(player_name: &str) -> i32 {
    let normalized = player_name.to_ascii_lowercase();

    let config = get_config().unwrap();
    let score = config.priority_list.iter().find_map(|(app_identifier, s)| {
        if normalized.contains(app_identifier) {
            Some(*s)
        } else {
            None
        }
    });

    score.unwrap_or(5) // Default score for apps not in the list
}

// Combines playback state and app name priorities, with playback state as the primary factor
fn total_priority_score(app_id: &str, state: &str, has_track: bool) -> i32 {
    let playback_score = playback_priority(state);
    let app_score = app_priority(app_id);
    let track_bonus = if has_track { 1 } else { 0 };

    // Keep playback state as the primary sort key, then app name preference.
    (playback_score * 1000) + (app_score * 10) + track_bonus
}

async fn refresh_cached_prioritized_media(source: &PlatformMediaSource) -> std::result::Result<(), ControllerError> {
    let media_sessions = tokio::time::timeout(SESSION_QUERY_TIMEOUT, get_current(source))
        .await
        .map_err(|_| ControllerError::Unresponsive("Timed out querying media sessions".to_string()))?
        .map_err(|error| ControllerError::Failed(error.to_string()))?;
    let best = media_sessions
        .into_iter()
        .max_by_key(|m| m.score)
        .unwrap_or_else(MediaInfo::empty);

    // Mark healthy under the lock so a fallback result can't overwrite this one.
    let changed = match latest_media_store().write() {
        Ok(mut latest) => {
            SESSION_SERVICE_HEALTHY.store(true, Ordering::Release);
            let changed = *latest != best;
            *latest = best;
            changed
        }
        Err(_) => false,
    };
    // Only wake the GUI when something it shows actually changed.
    if changed {
        mark_media_update_pending();
    }

    Ok(())
}

// Listens for media events until the connection to the session service fails.
async fn media_controller(source: &PlatformMediaSource) -> ControllerError {
    let mut stream = match source.event_stream().await {
        Ok(stream) => stream,
        Err(error) => return ControllerError::Failed(error.to_string()),
    };

    // Prime the cache once so the GUI can render data without blocking.
    if let Err(error) = refresh_cached_prioritized_media(source).await {
        return error;
    }

    let mut health_check = tokio::time::interval(HEALTH_CHECK_INTERVAL);
    health_check.reset();

    loop {
        tokio::select! {
            event = stream.next() => {
                let Some(event) = event else {
                    return ControllerError::Failed("Media event stream ended".to_string());
                };

                // Skip position and volume changes as the priority doesn't change.
                if let MediaEvent::PositionChanged { .. } = event {
                    continue;
                }
                if let MediaEvent::VolumeChanged { .. } = event {
                    continue;
                }
            }
            _ = health_check.tick() => {}
        }

        if let Err(error) = refresh_cached_prioritized_media(source).await {
            return error;
        }
    }
}

// Runs the media controller forever, reconnecting if the session service fails and restarting the service if it hangs.
pub async fn run_media_controller() {
    let mut pending_build: Option<JoinHandle<Result<PlatformMediaSource>>> = None;
    let mut unresponsive_since: Option<Instant> = None;
    let mut last_service_restart: Option<Instant> = None;

    loop {
        let build = pending_build.get_or_insert_with(|| tokio::task::spawn_blocking(build_media_source));

        // Connecting blocks until the service answers, which can be forever if it is hung.
        let error = match tokio::time::timeout(SESSION_QUERY_TIMEOUT, build).await {
            // Keep waiting on the same attempt rather than piling up new ones against a hung service.
            Err(_) => ControllerError::Unresponsive("Timed out connecting to media session service".to_string()),
            Ok(joined) => {
                pending_build = None;
                match joined {
                    Ok(Ok(source)) => media_controller(&source).await,
                    Ok(Err(error)) => ControllerError::Unresponsive(error.to_string()),
                    Err(join_error) => ControllerError::Failed(join_error.to_string()),
                }
            }
        };

        SESSION_SERVICE_HEALTHY.store(false, Ordering::Release);
        fallback::wake();
        match &error {
            ControllerError::Unresponsive(message) => {
                eprintln!("Media session service unresponsive: {message}");
                unresponsive_since.get_or_insert_with(Instant::now);
            }
            ControllerError::Failed(message) => {
                eprintln!("Media controller error: {message}");
                unresponsive_since = None;
            }
        }

        let hung = unresponsive_since.is_some_and(|since| since.elapsed() >= SERVICE_HANG_THRESHOLD);
        let cooled_down = last_service_restart.is_none_or(|at| at.elapsed() >= SERVICE_RESTART_COOLDOWN);
        if hung && cooled_down {
            last_service_restart = Some(Instant::now());
            unresponsive_since = None;
            match tokio::task::spawn_blocking(service_recovery::restart_media_service).await {
                Ok(Ok(())) => {
                    eprintln!("Restarted hung media session service");
                    // The stuck connection attempt belongs to the old service process, so start fresh.
                    pending_build = None;
                }
                Ok(Err(error)) => eprintln!("Failed to restart media session service: {error}"),
                Err(join_error) => eprintln!("Media session service restart task failed: {join_error}"),
            }
        }

        tokio::time::sleep(CONTROLLER_RETRY_DELAY).await;
    }
}