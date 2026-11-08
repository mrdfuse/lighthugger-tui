use crate::api::{self, Channel, Song};
use crate::audio::{Audio, State};
use crate::config::{self, Settings};
use crate::viz::{Analyzer, Visualizer, MODES};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

const CHANNEL_REFRESH: Duration = Duration::from_secs(60);
const SONG_REFRESH: Duration = Duration::from_secs(20);
const FAVORITES_CHECK: Duration = Duration::from_secs(2);

pub enum Message {
    Channels(Result<Vec<Channel>, String>),
    Songs(String, Vec<Song>),
}

pub struct App {
    pub channels: Vec<Channel>,
    pub favorites: Vec<String>,
    pub settings: Settings,
    pub visible: Vec<usize>,
    pub selected: usize,
    pub offset: usize,
    pub search: String,
    pub searching: bool,
    pub show_help: bool,
    pub should_quit: bool,
    pub network_error: Option<String>,
    pub notice: Option<String>,
    pub playing: Option<String>,
    pub songs_for: Option<String>,
    pub songs: Vec<Song>,
    pub state: State,
    pub server: String,
    pub buffered: f32,
    pub analyzer: Analyzer,
    pub visualizer: Visualizer,
    audio: Option<Audio>,
    audio_error: Option<String>,
    samples: Vec<(f32, f32)>,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    last_channels: Instant,
    last_songs: Option<Instant>,
    last_favorites_check: Instant,
    favorites_mtime: Option<std::time::SystemTime>,
    last_tick: Instant,
}

impl App {
    pub fn new(settings: Settings, audio: Result<Audio, String>) -> App {
        let (tx, rx) = channel();
        let (audio, audio_error) = match audio {
            Ok(audio) => (Some(audio), None),
            Err(e) => (None, Some(e)),
        };
        let app = App {
            channels: Vec::new(),
            favorites: config::load_favorites(),
            settings,
            visible: Vec::new(),
            selected: 0,
            offset: 0,
            search: String::new(),
            searching: false,
            show_help: false,
            should_quit: false,
            network_error: None,
            notice: None,
            playing: None,
            songs_for: None,
            songs: Vec::new(),
            state: State::Stopped,
            server: String::new(),
            buffered: 0.0,
            analyzer: Analyzer::new(),
            visualizer: Visualizer::new(),
            audio,
            audio_error,
            samples: Vec::new(),
            tx,
            rx,
            last_channels: Instant::now(),
            last_songs: None,
            last_favorites_check: Instant::now(),
            favorites_mtime: config::favorites_mtime(),
            last_tick: Instant::now(),
        };
        app.apply_volume();
        app
    }

    /// Shows the cached list right away, then loads a fresh one.
    pub fn start(&mut self) {
        if let Ok(json) = std::fs::read_to_string(config::cache_dir().join("channels.json")) {
            if let Ok(channels) = api::parse_channels(&json) {
                self.set_channels(channels);
            }
        }
        self.refresh_channels();
    }

    pub fn refresh_channels(&mut self) {
        self.last_channels = Instant::now();
        let tx = self.tx.clone();
        thread::spawn(move || {
            let result = api::fetch_channels().map(|(channels, json)| {
                let dir = config::cache_dir();
                if std::fs::create_dir_all(&dir).is_ok() {
                    let _ = std::fs::write(dir.join("channels.json"), json);
                }
                channels
            });
            let _ = tx.send(Message::Channels(result));
        });
    }

    pub fn set_channels(&mut self, channels: Vec<Channel>) {
        let current = self.selected_channel().map(|c| c.id.clone());
        self.channels = channels;
        if current.is_none() && !self.settings.last_station.is_empty() {
            self.rebuild(Some(self.settings.last_station.clone()));
        } else {
            self.rebuild(current);
        }
    }

    pub fn channel(&self, id: &str) -> Option<&Channel> {
        self.channels.iter().find(|c| c.id == id)
    }

    pub fn selected_channel(&self) -> Option<&Channel> {
        self.visible.get(self.selected).and_then(|i| self.channels.get(*i))
    }

    pub fn playing_channel(&self) -> Option<&Channel> {
        self.playing.as_deref().and_then(|id| self.channel(id))
    }

    pub fn is_favorite(&self, id: &str) -> bool {
        self.favorites.iter().any(|f| f == id)
    }

    /// All, favorites and one entry per genre; tab cycles through them.
    pub fn filters(&self) -> Vec<String> {
        let mut genres: Vec<&str> = self.channels.iter().flat_map(|c| c.genres()).collect();
        genres.sort_unstable();
        genres.dedup();
        let mut filters = vec!["favorites".to_string(), "all".to_string()];
        filters.extend(genres.into_iter().map(str::to_string));
        filters
    }

    fn matches(&self, channel: &Channel) -> bool {
        let filter = self.settings.filter.as_str();
        let in_filter = match filter {
            "favorites" => self.is_favorite(&channel.id),
            "all" => true,
            genre => channel.genres().any(|g| g == genre),
        };
        if !in_filter {
            return false;
        }
        let needle = self.search.trim().to_lowercase();
        needle.is_empty()
            || [&channel.title, &channel.description, &channel.genre, &channel.dj, &channel.last_playing]
                .iter()
                .any(|field| field.to_lowercase().contains(&needle))
    }

    /// Recomputes the list and keeps `keep` selected when it is still in there.
    pub fn rebuild(&mut self, keep: Option<String>) {
        let mut visible: Vec<usize> = (0..self.channels.len()).filter(|i| self.matches(&self.channels[*i])).collect();
        let channels = &self.channels;
        let favorites = &self.favorites;
        let by_listeners = self.settings.sort_listeners;
        visible.sort_by(|a, b| {
            let (a, b) = (&channels[*a], &channels[*b]);
            let fav = |c: &Channel| !favorites.contains(&c.id);
            fav(a)
                .cmp(&fav(b))
                .then_with(|| if by_listeners { b.listener_count().cmp(&a.listener_count()) } else { std::cmp::Ordering::Equal })
                .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        });
        self.visible = visible;
        self.selected = keep
            .and_then(|id| self.visible.iter().position(|i| self.channels[*i].id == id))
            .unwrap_or_else(|| self.selected.min(self.visible.len().saturating_sub(1)));
    }

    fn keep_selection(&mut self) {
        let keep = self.selected_channel().map(|c| c.id.clone());
        self.rebuild(keep);
    }

    fn apply_volume(&self) {
        if let Some(audio) = &self.audio {
            audio.set_volume(self.settings.volume, self.settings.muted);
        }
    }

    pub fn play(&mut self, id: &str) {
        let Some(channel) = self.channel(id).cloned() else { return };
        self.settings.last_station = channel.id.clone();
        self.playing = Some(channel.id.clone());
        self.settings.save();
        self.rebuild(Some(channel.id.clone()));
        match (&self.audio, api::playlist_for(&channel, self.settings.quality)) {
            (Some(audio), Some(playlist)) => audio.play(playlist.url.clone()),
            (None, _) => self.state = State::Error(self.audio_error.clone().unwrap_or_else(|| "no audio output".into())),
            (_, None) => self.state = State::Error("this station has no streams available".into()),
        }
    }

    pub fn stop(&mut self) {
        if let Some(audio) = &self.audio {
            audio.stop();
        }
        self.playing = None;
        self.state = State::Stopped;
    }

    fn toggle(&mut self) {
        if self.playing.is_some() {
            self.stop();
        } else if let Some(id) = self.selected_channel().map(|c| c.id.clone()) {
            self.play(&id);
        }
    }

    /// Next/previous cycle through the favorites when there are any.
    fn step_station(&mut self, delta: i32) {
        let mut ids: Vec<&Channel> = self.channels.iter().filter(|c| self.favorites.is_empty() || self.is_favorite(&c.id)).collect();
        ids.sort_by_key(|c| c.title.to_lowercase());
        if ids.is_empty() {
            return;
        }
        let reference = self.playing.clone().or_else(|| self.selected_channel().map(|c| c.id.clone())).or_else(|| Some(self.settings.last_station.clone()));
        let current = reference.and_then(|id| ids.iter().position(|c| c.id == id));
        let n = ids.len() as i32;
        let next = match current {
            Some(i) => (i as i32 + delta).rem_euclid(n),
            None if delta > 0 => 0,
            None => n - 1,
        };
        let id = ids[next as usize].id.clone();
        self.play(&id);
    }

    fn toggle_favorite(&mut self) {
        let Some(id) = self.selected_channel().map(|c| c.id.clone()) else { return };
        // Start from the file: the KDE app may have changed it in the meantime.
        self.favorites = config::load_favorites();
        if let Some(position) = self.favorites.iter().position(|f| *f == id) {
            self.favorites.remove(position);
        } else {
            self.favorites.push(id.clone());
        }
        if let Err(e) = config::save_favorites(&self.favorites) {
            self.network_error = Some(format!("could not save favorites: {e}"));
        }
        self.favorites_mtime = config::favorites_mtime();
        self.rebuild(Some(id));
    }

    fn cycle_filter(&mut self, delta: i32) {
        let filters = self.filters();
        let current = filters.iter().position(|f| *f == self.settings.filter).unwrap_or(0) as i32;
        self.settings.filter = filters[(current + delta).rem_euclid(filters.len() as i32) as usize].clone();
        self.settings.save();
        self.keep_selection();
    }

    fn change_quality(&mut self) {
        self.settings.quality = (self.settings.quality + 1) % api::QUALITIES.len();
        self.settings.save();
        if let Some(id) = self.playing.clone() {
            self.play(&id);
        }
    }

    fn change_volume(&mut self, delta: f32) {
        self.settings.volume = (self.settings.volume + delta).clamp(0.0, 1.0);
        self.settings.muted = false;
        self.settings.stored_muted = None;
        self.apply_volume();
        self.settings.save();
    }

    fn move_selection(&mut self, delta: i32) {
        if self.visible.is_empty() {
            return;
        }
        let last = self.visible.len() as i32 - 1;
        self.selected = (self.selected as i32 + delta).clamp(0, last) as usize;
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.notice = None;
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
            self.should_quit = true;
            return;
        }
        if self.show_help {
            self.show_help = false;
            return;
        }
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.search.clear();
                    self.searching = false;
                    self.keep_selection();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.search.pop();
                    self.keep_selection();
                }
                KeyCode::Up => self.move_selection(-1),
                KeyCode::Down => self.move_selection(1),
                KeyCode::Char(c) => {
                    self.search.push(c);
                    self.selected = 0;
                    self.rebuild(None);
                }
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.search.is_empty() {
                    self.should_quit = true;
                } else {
                    self.search.clear();
                    self.keep_selection();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(1),
            KeyCode::PageUp => self.move_selection(-10),
            KeyCode::PageDown => self.move_selection(10),
            KeyCode::Home | KeyCode::Char('g') => self.selected = 0,
            KeyCode::End | KeyCode::Char('G') => self.selected = self.visible.len().saturating_sub(1),
            KeyCode::Enter => {
                if let Some(id) = self.selected_channel().map(|c| c.id.clone()) {
                    self.play(&id);
                }
            }
            KeyCode::Char(' ') => self.toggle(),
            KeyCode::Char('f') => self.toggle_favorite(),
            KeyCode::Char('n') => self.step_station(1),
            KeyCode::Char('p') => self.step_station(-1),
            KeyCode::Char('q') => self.change_quality(),
            KeyCode::Char('v') => {
                let current = MODES.iter().position(|m| *m == self.settings.viz_mode).unwrap_or(0);
                self.settings.viz_mode = MODES[(current + 1) % MODES.len()].to_string();
                self.settings.show_viz = true;
                self.settings.stored_show_viz = None;
                self.settings.save();
            }
            KeyCode::Char('h') | KeyCode::Char('3') => {
                self.settings.show_viz = !self.settings.show_viz;
                self.settings.stored_show_viz = None;
                self.settings.save();
            }
            KeyCode::Char('2') => {
                self.settings.show_info = !self.settings.show_info;
                self.settings.save();
            }
            KeyCode::Char('5') => {
                self.settings.show_history = !self.settings.show_history;
                self.settings.save();
            }
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right => self.change_volume(0.05),
            KeyCode::Char('-') | KeyCode::Left => self.change_volume(-0.05),
            KeyCode::Char('m') => {
                self.settings.muted = !self.settings.muted;
                self.settings.stored_muted = None;
                self.apply_volume();
                self.settings.save();
            }
            KeyCode::Char('s') => {
                self.settings.sort_listeners = !self.settings.sort_listeners;
                self.settings.save();
                self.keep_selection();
            }
            KeyCode::Tab => self.cycle_filter(1),
            KeyCode::BackTab => self.cycle_filter(-1),
            KeyCode::Char('/') => self.searching = true,
            KeyCode::Char('r') => self.refresh_channels(),
            KeyCode::Char('?') => self.show_help = true,
            _ => {}
        }
    }

    /// The station whose song history is shown: what plays, else what is selected.
    pub fn focus_id(&self) -> Option<String> {
        self.playing.clone().or_else(|| self.selected_channel().map(|c| c.id.clone()))
    }

    fn request_songs(&mut self, id: String) {
        self.last_songs = Some(Instant::now());
        let tx = self.tx.clone();
        thread::spawn(move || {
            if let Ok(songs) = api::fetch_songs(&id) {
                let _ = tx.send(Message::Songs(id, songs));
            }
        });
    }

    pub fn is_animating(&self) -> bool {
        self.settings.show_viz || self.playing.is_some()
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.25);
        self.last_tick = now;

        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Channels(Ok(channels)) => {
                    self.network_error = None;
                    self.set_channels(channels);
                }
                Message::Channels(Err(e)) => self.network_error = Some(e),
                Message::Songs(id, songs) => {
                    if self.focus_id().as_deref() == Some(id.as_str()) {
                        self.songs_for = Some(id);
                        self.songs = songs;
                    }
                }
            }
        }

        if now.duration_since(self.last_channels) >= CHANNEL_REFRESH {
            self.refresh_channels();
        }

        let focus = self.focus_id();
        let changed = focus != self.songs_for;
        let since = self.last_songs.map(|t| now.duration_since(t));
        let due = match since {
            None => true,
            Some(elapsed) => elapsed >= SONG_REFRESH || (changed && elapsed >= Duration::from_millis(300)),
        };
        if let (Some(id), true) = (focus, due) {
            self.request_songs(id);
        }

        if now.duration_since(self.last_favorites_check) >= FAVORITES_CHECK {
            self.last_favorites_check = now;
            let mtime = config::favorites_mtime();
            if mtime != self.favorites_mtime {
                self.favorites_mtime = mtime;
                self.favorites = config::load_favorites();
                self.keep_selection();
            }
        }

        let mut rate = 48000;
        if let Some(audio) = &self.audio {
            rate = audio.sample_rate();
            let snapshot = audio.snapshot();
            self.state = snapshot.state;
            self.server = snapshot.server;
            self.buffered = snapshot.buffered;
            audio.copy_analysis(&mut self.samples);
        }
        if self.state != State::Playing {
            self.samples.clear();
        }
        self.analyzer.update(&self.samples, rate, dt);
        self.visualizer.advance(&self.analyzer, dt);
    }

    /// Stations for the tests; never touches the network or sound card.
    #[cfg(test)]
    pub fn for_tests(channels: Vec<Channel>) -> App {
        // Keep tests away from the real favorites and settings.
        let sandbox = std::env::temp_dir().join(format!("lighthugger-tui-test-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &sandbox);
        std::env::set_var("XDG_CACHE_HOME", &sandbox);
        let mut app = App::new(Settings::default(), Err("no audio in tests".into()));
        app.favorites = vec!["a".into()];
        app.set_channels(channels);
        app
    }

    pub fn clock_time(timestamp: i64) -> String {
        use chrono::{Local, TimeZone};
        Local.timestamp_opt(timestamp, 0).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default()
    }

    pub fn now_timestamp() -> i64 {
        chrono::Local::now().timestamp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    fn station(id: &str, title: &str, genre: &str, listeners: &str) -> Channel {
        Channel { id: id.into(), title: title.into(), genre: genre.into(), listeners: listeners.into(), ..Default::default() }
    }

    fn app() -> App {
        App::for_tests(vec![station("a", "Zulu", "ambient", "5"), station("b", "Alpha", "ambient|jazz", "50"), station("c", "Mike", "metal", "500")])
    }

    fn titles(app: &App) -> Vec<&str> {
        app.visible.iter().map(|i| app.channels[*i].title.as_str()).collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    #[test]
    fn favorites_filter_only_shows_favorites() {
        let app = app();
        assert_eq!(titles(&app), vec!["Zulu"]);
    }

    #[test]
    fn all_lists_favorites_first_and_can_sort_by_listeners() {
        let mut app = app();
        app.settings.filter = "all".into();
        app.rebuild(None);
        assert_eq!(titles(&app), vec!["Zulu", "Alpha", "Mike"]);
        app.settings.sort_listeners = true;
        app.rebuild(None);
        assert_eq!(titles(&app), vec!["Zulu", "Mike", "Alpha"]);
    }

    #[test]
    fn genres_and_search_narrow_the_list() {
        let mut app = app();
        app.settings.filter = "jazz".into();
        app.rebuild(None);
        assert_eq!(titles(&app), vec!["Alpha"]);
        app.settings.filter = "all".into();
        press(&mut app, KeyCode::Char('/'));
        press(&mut app, KeyCode::Char('m'));
        press(&mut app, KeyCode::Char('i'));
        assert_eq!(titles(&app), vec!["Mike"]);
        assert!(app.settings.show_viz, "typing a search must not trigger shortcuts");
        press(&mut app, KeyCode::Esc);
        assert_eq!(titles(&app).len(), 3);
        assert!(!app.should_quit, "esc only leaves the search");
    }

    #[test]
    fn selection_survives_a_refresh_and_stays_in_range() {
        let mut app = app();
        app.settings.filter = "all".into();
        app.rebuild(None);
        press(&mut app, KeyCode::Down);
        assert_eq!(app.selected_channel().unwrap().id, "b");
        app.set_channels(vec![station("c", "Mike", "metal", "1"), station("b", "Alpha", "jazz", "2")]);
        assert_eq!(app.selected_channel().unwrap().id, "b");
        for _ in 0..10 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.selected_channel().unwrap().id, "c");
    }

    #[test]
    fn hiding_the_visualizer_is_a_toggle_and_v_shows_it_again() {
        let mut app = app();
        assert_eq!(app.settings.viz_mode, "ship");
        press(&mut app, KeyCode::Char('h'));
        assert!(!app.settings.show_viz);
        press(&mut app, KeyCode::Char('v'));
        assert!(app.settings.show_viz);
        assert_eq!(app.settings.viz_mode, "spectrum");
    }

    #[test]
    fn playing_without_audio_reports_the_reason() {
        let mut app = app();
        app.play("a");
        assert_eq!(app.state, State::Error("no audio in tests".into()));
    }
}
