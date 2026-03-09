use crate::config;
use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;

#[derive(Clone)]
pub struct MediaInfo {
    pub app_id: String,
    pub title: String,
    pub artist: String,
}

impl MediaInfo {
    pub fn new(app_id: String, title: String, artist: String) -> Self {
        Self { app_id, title, artist }
    }
}

// Get the current media sessions and their info
fn get_current() -> Result<Vec<MediaInfo>, windows::core::Error> {
    // Get the media session manager
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
    let mut results = Vec::<MediaInfo>::new();

    // Try to get current sessions and their properties
    let sessions = manager.GetSessions()?;
    for s in &sessions {
        let properties = s.TryGetMediaPropertiesAsync()?.join()?;
        let app_id = s.SourceAppUserModelId()?.to_string();
        let title = properties.Title()?.to_string();
        let artist = properties.Artist()?.to_string();

        if app_id.is_empty() || title.is_empty() || artist.is_empty() {
            continue;
        }

        results.push(MediaInfo::new(app_id, title, artist));
    }

    Ok(results)
}

// Determine the priority rank of an app_id based on the priority list
fn priority_rank(app_id: &str, priority_list: &[String]) -> Option<usize> {
    let app_id_lower = app_id.to_ascii_lowercase();
    priority_list
        .iter()
        .position(|p| app_id_lower.contains(&p.to_ascii_lowercase()))
}

// Pick the media session with the highest priority based on the provided list
pub fn get_prioritized_media() -> Option<MediaInfo> {
    let sessions = get_current().ok()?;
    let priority_list = config::get_config().map(|cfg| cfg.priority_list.as_slice());

    sessions
        .iter()
        .filter_map(|session| {
            priority_list
                .and_then(|list| priority_rank(&session.app_id, list))
                .map(|rank| (rank, session))
        })
        .min_by_key(|(rank, _)| *rank)
        .map(|(_, session)| (*session).clone())
        .or_else(|| sessions.first().cloned())
}
