use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn home_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

pub fn config_dir() -> PathBuf {
    home_dir("XDG_CONFIG_HOME", ".config").join("lighthugger")
}

pub fn cache_dir() -> PathBuf {
    home_dir("XDG_CACHE_HOME", ".cache").join("lighthugger-tui")
}

/// Written through a rename so the KDE app never reads a half-written file.
fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    let mut file = fs::File::create(&tmp)?;
    file.write_all(content.as_bytes())?;
    file.sync_all()?;
    fs::rename(tmp, path)
}

/// Favorites live in the KDE app's QSettings file, so both clients share them.
pub fn favorites_path() -> PathBuf {
    config_dir().join("lighthugger.conf")
}

pub fn parse_favorites(conf: &str) -> Vec<String> {
    let mut in_general = false;
    for line in conf.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_general = line == "[General]";
        } else if in_general {
            if let Some(value) = line.strip_prefix("favorites=") {
                if value == "@Invalid()" {
                    return Vec::new();
                }
                return value
                    .split(',')
                    .map(|id| id.trim().trim_matches('"').to_string())
                    .filter(|id| !id.is_empty())
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Replaces only the favorites key and keeps everything the KDE app stored.
pub fn with_favorites(conf: &str, favorites: &[String]) -> String {
    let value = if favorites.is_empty() { "@Invalid()".to_string() } else { favorites.join(", ") };
    let entry = format!("favorites={value}");
    let mut lines: Vec<String> = Vec::new();
    let mut in_general = false;
    let mut has_general = false;
    let mut done = false;
    for line in conf.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            if in_general && !done {
                lines.push(entry.clone());
                done = true;
            }
            in_general = trimmed == "[General]";
            has_general |= in_general;
        } else if in_general && trimmed.starts_with("favorites=") {
            if !done {
                lines.push(entry.clone());
                done = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if !done {
        if has_general {
            lines.push(entry);
        } else {
            let mut general = vec!["[General]".to_string(), entry];
            if !lines.is_empty() {
                general.push(String::new());
            }
            general.append(&mut lines);
            lines = general;
        }
    }
    lines.join("\n") + "\n"
}

pub fn load_favorites() -> Vec<String> {
    fs::read_to_string(favorites_path()).map(|c| parse_favorites(&c)).unwrap_or_default()
}

pub fn save_favorites(favorites: &[String]) -> std::io::Result<()> {
    let path = favorites_path();
    let current = fs::read_to_string(&path).unwrap_or_default();
    write_atomic(&path, &with_favorites(&current, favorites))
}

pub fn favorites_mtime() -> Option<std::time::SystemTime> {
    fs::metadata(favorites_path()).and_then(|m| m.modified()).ok()
}

#[derive(Debug, Clone)]
pub struct Settings {
    pub volume: f32,
    pub muted: bool,
    pub quality: usize,
    pub viz_mode: String,
    pub show_viz: bool,
    pub show_info: bool,
    pub show_history: bool,
    pub filter: String,
    pub sort_listeners: bool,
    pub last_station: String,
    /// What to store while a command-line option overrides the value for one run.
    pub stored_show_viz: Option<bool>,
    pub stored_muted: Option<bool>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            volume: 0.8,
            muted: false,
            quality: 1,
            viz_mode: "ship".into(),
            show_viz: true,
            show_info: true,
            show_history: true,
            filter: "favorites".into(),
            sort_listeners: false,
            last_station: String::new(),
            stored_show_viz: None,
            stored_muted: None,
        }
    }
}

impl Settings {
    fn path() -> PathBuf {
        config_dir().join("tui.conf")
    }

    pub fn parse(text: &str) -> Settings {
        let map: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim(), v.trim())).collect();
        let d = Settings::default();
        let flag = |key: &str, default: bool| map.get(key).map(|v| *v == "true").unwrap_or(default);
        Settings {
            volume: map.get("volume").and_then(|v| v.parse().ok()).unwrap_or(d.volume).clamp(0.0, 1.0),
            muted: flag("muted", d.muted),
            quality: map.get("quality").and_then(|v| v.parse().ok()).unwrap_or(d.quality).min(3),
            viz_mode: map.get("viz_mode").map(|v| v.to_string()).unwrap_or(d.viz_mode),
            show_viz: flag("show_viz", d.show_viz),
            show_info: flag("show_info", d.show_info),
            show_history: flag("show_history", d.show_history),
            filter: map.get("filter").map(|v| v.to_string()).unwrap_or(d.filter),
            sort_listeners: flag("sort_listeners", d.sort_listeners),
            last_station: map.get("last_station").map(|v| v.to_string()).unwrap_or(d.last_station),
            stored_show_viz: None,
            stored_muted: None,
        }
    }

    pub fn load() -> Settings {
        fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) {
        let text = format!(
            "volume={}\nmuted={}\nquality={}\nviz_mode={}\nshow_viz={}\nshow_info={}\nshow_history={}\nfilter={}\nsort_listeners={}\nlast_station={}\n",
            self.volume, self.stored_muted.unwrap_or(self.muted), self.quality, self.viz_mode, self.stored_show_viz.unwrap_or(self.show_viz), self.show_info, self.show_history, self.filter, self.sort_listeners, self.last_station
        );
        let _ = write_atomic(&Self::path(), &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const KDE: &str = "[General]\nfavorites=defcon, missioncontrol, darkzone\nlastChannel=defcon\nquality=1\nvolume=0.77\n";

    #[test]
    fn reads_qsettings_string_lists() {
        assert_eq!(parse_favorites(KDE), vec!["defcon", "missioncontrol", "darkzone"]);
        assert_eq!(parse_favorites("[General]\nfavorites=groovesalad\n"), vec!["groovesalad"]);
        assert!(parse_favorites("[General]\nfavorites=@Invalid()\n").is_empty());
        assert!(parse_favorites("").is_empty());
        assert!(parse_favorites("[Other]\nfavorites=x\n").is_empty());
    }

    #[test]
    fn keeps_the_settings_of_the_kde_app() {
        let out = with_favorites(KDE, &["lush".to_string(), "defcon".to_string()]);
        assert_eq!(out, "[General]\nfavorites=lush, defcon\nlastChannel=defcon\nquality=1\nvolume=0.77\n");
        assert_eq!(parse_favorites(&with_favorites(KDE, &[])), Vec::<String>::new());
        assert!(with_favorites(KDE, &[]).contains("favorites=@Invalid()"));
    }

    #[test]
    fn adds_the_key_when_it_is_missing() {
        assert_eq!(with_favorites("", &["a".into()]), "[General]\nfavorites=a\n");
        assert_eq!(with_favorites("[General]\nvolume=1\n", &["a".into()]), "[General]\nvolume=1\nfavorites=a\n");
        assert_eq!(with_favorites("[General]\nvolume=1\n\n[X]\ny=2\n", &["a".into()]), "[General]\nvolume=1\n\nfavorites=a\n[X]\ny=2\n");
        assert_eq!(with_favorites("[X]\ny=2\n", &["a".into()]), "[General]\nfavorites=a\n\n[X]\ny=2\n");
    }

    #[test]
    fn settings_round_trip_with_defaults() {
        let s = Settings::parse("");
        assert_eq!(s.viz_mode, "ship");
        assert!(s.show_viz);
        let s = Settings::parse("show_viz=false\nvolume=4\nquality=9\n");
        assert!(!s.show_viz);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.quality, 3);
    }
}
