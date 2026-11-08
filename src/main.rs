mod api;
mod app;
mod audio;
mod config;
mod ui;
mod viz;

use app::App;
use audio::Audio;
use config::Settings;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use std::time::Duration;

const HELP: &str = "\
Lighthugger TUI: a terminal player for SomaFM

Usage: lighthugger-tui [options] [station-id]

  station-id     start playing this station, for example dronezone
  --no-viz       start with the visualizer hidden (h toggles it)
  --mute         start muted
  -h, --help     show this help
  -V, --version  show the version

Press ? in the app for all keys.
";

/// Sound libraries print to stderr, which would tear up the screen. Sends it to
/// a log and returns the real stderr, so a crash report can still be shown.
fn redirect_stderr() -> Option<i32> {
    use std::os::fd::IntoRawFd;
    let dir = config::cache_dir();
    std::fs::create_dir_all(&dir).ok()?;
    let log = std::fs::File::create(dir.join("stderr.log")).ok()?;
    unsafe {
        let saved = libc::dup(2);
        if saved < 0 {
            return None;
        }
        libc::dup2(log.into_raw_fd(), 2);
        Some(saved)
    }
}

fn main() {
    let mut station = None;
    let mut settings = Settings::load();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-h" | "--help" => return print!("{HELP}"),
            "-V" | "--version" => return println!("lighthugger-tui {}", env!("CARGO_PKG_VERSION")),
            "--no-viz" => {
                settings.stored_show_viz = Some(settings.show_viz);
                settings.show_viz = false;
            }
            "--mute" => {
                settings.stored_muted = Some(settings.muted);
                settings.muted = true;
            }
            other if other.starts_with('-') => {
                eprint!("unknown option {other}\n\n{HELP}");
                std::process::exit(2);
            }
            other => station = Some(other.to_string()),
        }
    }

    let saved_stderr = redirect_stderr();
    let previous_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        if let Some(fd) = saved_stderr {
            unsafe { libc::dup2(fd, 2) };
        }
        previous_hook(info);
    }));

    let mut terminal = ratatui::init();
    let mut app = App::new(settings, Audio::new());
    app.start();

    let mut pending_station = station;
    while !app.should_quit {
        if let Some(id) = pending_station.as_deref() {
            if app.channel(id).is_some() {
                let id = id.to_string();
                app.play(&id);
                pending_station = None;
            } else if !app.channels.is_empty() {
                app.notice = Some(format!("unknown station: {id}"));
                pending_station = None;
            }
        }
        app.tick();
        if terminal.draw(|frame| ui::draw(frame, &mut app)).is_err() {
            break;
        }
        let wait = if app.is_animating() { Duration::from_millis(33) } else { Duration::from_millis(250) };
        if event::poll(wait).unwrap_or(false) {
            // Handle everything that is waiting, so held keys never lag behind.
            while event::poll(Duration::ZERO).unwrap_or(false) {
                match event::read() {
                    Ok(Event::Key(key)) if key.kind != KeyEventKind::Release => app.handle_key(key),
                    Ok(_) => {}
                    Err(_) => app.should_quit = true,
                }
            }
        }
    }

    app.settings.save();
    app.stop();
    ratatui::restore();
}
