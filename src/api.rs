use serde::Deserialize;
use std::time::Duration;

pub const USER_AGENT: &str = "Lighthugger-TUI/0.1 (+https://github.com/mrdfuse/lighthugger-tui)";

#[derive(Debug, Clone, Deserialize)]
pub struct Playlist {
    pub url: String,
    pub format: String,
    pub quality: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Channel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub dj: String,
    #[serde(default)]
    pub genre: String,
    #[serde(default)]
    pub listeners: String,
    #[serde(default, rename = "lastPlaying")]
    pub last_playing: String,
    #[serde(default)]
    pub playlists: Vec<Playlist>,
}

impl Channel {
    pub fn listener_count(&self) -> u32 {
        self.listeners.parse().unwrap_or(0)
    }

    pub fn genres(&self) -> impl Iterator<Item = &str> {
        self.genre.split('|').filter(|g| !g.is_empty())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Song {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub artist: String,
    #[serde(default)]
    pub album: String,
    #[serde(default)]
    pub date: String,
}

impl Song {
    pub fn timestamp(&self) -> Option<i64> {
        self.date.parse().ok()
    }
}

pub const QUALITIES: [(&str, &str, &str); 4] = [
    ("MP3 128k", "mp3", "highest"),
    ("AAC 128k", "aac", "highest"),
    ("AAC+ 64k", "aacp", "high"),
    ("AAC+ 32k", "aacp", "low"),
];

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .user_agent(USER_AGENT)
        .timeout(Duration::from_secs(15))
        .build()
}

#[derive(Deserialize)]
struct ChannelList {
    channels: Vec<Channel>,
}

#[derive(Deserialize)]
struct SongList {
    songs: Vec<Song>,
}

pub fn parse_channels(json: &str) -> Result<Vec<Channel>, String> {
    let list: ChannelList = serde_json::from_str(json).map_err(|e| e.to_string())?;
    if list.channels.is_empty() {
        return Err("empty station list".into());
    }
    Ok(list.channels)
}

/// Returns the raw JSON too, so it can be cached for the next start.
pub fn fetch_channels() -> Result<(Vec<Channel>, String), String> {
    let body = agent()
        .get("https://somafm.com/channels.json")
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    Ok((parse_channels(&body)?, body))
}

pub fn fetch_songs(id: &str) -> Result<Vec<Song>, String> {
    let list: SongList = agent()
        .get(&format!("https://somafm.com/songs/{id}.json"))
        .call()
        .map_err(|e| e.to_string())?
        .into_json()
        .map_err(|e| e.to_string())?;
    Ok(list.songs)
}

pub fn fetch_streams(playlist_url: &str) -> Result<Vec<String>, String> {
    let body = agent()
        .get(playlist_url)
        .call()
        .map_err(|e| e.to_string())?
        .into_string()
        .map_err(|e| e.to_string())?;
    let streams = parse_pls(&body);
    if streams.is_empty() {
        return Err("this station has no streams available".into());
    }
    Ok(streams)
}

pub fn parse_pls(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|line| {
            let (key, value) = line.trim().split_once('=')?;
            let key = key.to_ascii_lowercase();
            let number = key.strip_prefix("file")?;
            (!number.is_empty() && number.chars().all(|c| c.is_ascii_digit())).then(|| value.trim().to_string())
        })
        .collect()
}

/// Picks the playlist for a quality, falling back to the same format, then to anything.
pub fn playlist_for(channel: &Channel, quality: usize) -> Option<&Playlist> {
    let (_, format, level) = QUALITIES[quality.min(QUALITIES.len() - 1)];
    channel
        .playlists
        .iter()
        .find(|p| p.format == format && p.quality == level)
        .or_else(|| channel.playlists.iter().find(|p| p.format == format))
        .or_else(|| channel.playlists.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pls_files() {
        let pls = "[playlist]\nnumberofentries=2\nFile1=https://a/x\nTitle1=A=B\nfile2= https://b/y \nLength1=-1\n";
        assert_eq!(parse_pls(pls), vec!["https://a/x", "https://b/y"]);
    }

    #[test]
    fn falls_back_when_quality_is_missing() {
        let mk = |format: &str, quality: &str| Playlist { url: format!("{format}-{quality}"), format: format.into(), quality: quality.into() };
        let channel = Channel { playlists: vec![mk("mp3", "highest"), mk("aacp", "high")], ..Default::default() };
        assert_eq!(playlist_for(&channel, 0).unwrap().url, "mp3-highest");
        assert_eq!(playlist_for(&channel, 3).unwrap().url, "aacp-high");
        assert_eq!(playlist_for(&channel, 1).unwrap().url, "mp3-highest");
    }
}
