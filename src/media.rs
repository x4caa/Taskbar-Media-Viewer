use crate::config::get_config;
use nowhear::source::PlatformMediaSource;
use nowhear::{MediaSource, MediaSourceBuilder, Result};
use std::sync::OnceLock;
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
};

#[derive(Clone)]
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

static MEDIA_SOURCE: OnceLock<PlatformMediaSource> = OnceLock::new();

// Gets the media session manager, which provides access to all current media sessions
fn get_session_manager() -> windows::core::Result<GlobalSystemMediaTransportControlsSessionManager>
{
    pollster::block_on(GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?)
}

fn find_session_by_app_id(
    app_id: &str,
) -> windows::core::Result<Option<GlobalSystemMediaTransportControlsSession>> {
    let manager = get_session_manager()?;
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
    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TryPauseAsync()?)
}

// Plays the session with the given app_id, if its playing
pub fn play_session(app_id: &str) -> windows::core::Result<bool> {
    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TryPlayAsync()?)
}

// Skips to the next track in the session with the given app_id, if supported
pub fn next_session(app_id: &str) -> windows::core::Result<bool> {
    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TrySkipNextAsync()?)
}

// Skip to the previous track in the session with the given app_id, if supported
pub fn previous_session(app_id: &str) -> windows::core::Result<bool> {
    let Some(session) = find_session_by_app_id(app_id)? else {
        return Ok(false);
    };

    pollster::block_on(session.TrySkipPreviousAsync()?)
}

// Gets the static media source, initializing it only once as constant building can break it
fn get_media_source() -> Result<&'static PlatformMediaSource> {
    if let Some(source) = MEDIA_SOURCE.get() {
        return Ok(source);
    }

    let built = pollster::block_on(MediaSourceBuilder::new().build())?;
    let _ = MEDIA_SOURCE.set(built);

    Ok(MEDIA_SOURCE
        .get()
        .expect("MEDIA_SOURCE should be initialized"))
}

// Get the current media sessions and their info
async fn get_current() -> Result<Vec<MediaInfo>> {
    let source = get_media_source()?;
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

// Pick the media session with the highest priority based on the provided list
pub fn get_prioritized_media() -> Result<MediaInfo> {
    let media_sessions = pollster::block_on(get_current())?;
    let best = media_sessions.into_iter().max_by_key(|m| m.score);

    Ok(best.unwrap_or_else(|| MediaInfo::empty()))
}
