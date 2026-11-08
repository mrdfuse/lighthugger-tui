use crate::api::QUALITIES;
use crate::app::App;
use crate::audio::State;
use crate::viz::gradient;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph, Wrap};
use ratatui::Frame;

const LINE: Color = Color::Rgb(59, 74, 99);
const TITLE: Color = Color::Rgb(127, 219, 202);
const DIM: Color = Color::Rgb(112, 127, 150);
const TEXT: Color = Color::Rgb(200, 211, 224);
const BRIGHT: Color = Color::Rgb(255, 255, 255);
const STAR: Color = Color::Rgb(255, 203, 107);
const SELECTED: Color = Color::Rgb(29, 43, 69);
const ERROR: Color = Color::Rgb(255, 95, 126);
const EMPTY_METER: Color = Color::Rgb(40, 50, 66);

fn fg(color: Color) -> Style {
    Style::default().fg(color)
}

fn bold(color: Color) -> Style {
    fg(color).add_modifier(Modifier::BOLD)
}

fn panel<'a>(key: &'a str, title: &'a str) -> Block<'a> {
    let mut spans = vec![Span::styled("┤", fg(LINE))];
    if !key.is_empty() {
        spans.push(Span::styled(key, bold(STAR)));
    }
    spans.push(Span::styled(title, bold(TITLE)));
    spans.push(Span::styled("├", fg(LINE)));
    Block::bordered().border_type(BorderType::Rounded).border_style(fg(LINE)).title(Line::from(spans))
}

fn truncate(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    text.chars().take(width - 1).chain(std::iter::once('…')).collect()
}

fn pad(text: &str, width: usize) -> String {
    format!("{:<width$}", truncate(text, width), width = width)
}

pub struct Panels {
    stations: Rect,
    info: Option<Rect>,
    viz: Option<Rect>,
    now: Rect,
    history: Option<Rect>,
    footer: Option<Rect>,
}

/// Decides what fits. Station list and now playing always stay; the rest goes
/// first when there is little room, so a small terminal stays usable.
pub fn layout(area: Rect, app: &App) -> Panels {
    let footer_height = u16::from(area.height >= 6);
    let [body, footer] = Layout::vertical([Constraint::Min(0), Constraint::Length(footer_height)]).areas(area);
    let footer = (footer_height > 0).then_some(footer);
    let settings = &app.settings;

    if area.width >= 72 && body.height >= 12 {
        let left_width = (area.width * 3 / 8).clamp(30, 40);
        let [left, right] = Layout::horizontal([Constraint::Length(left_width), Constraint::Min(0)]).areas(body);

        let show_info = settings.show_info && left.height >= 19;
        let info_height = if show_info { 7 } else { 0 };
        let [stations, info] = Layout::vertical([Constraint::Min(0), Constraint::Length(info_height)]).areas(left);

        let now_height = if right.height >= 16 { 7 } else { 4 };
        let rest = right.height - now_height;
        let history_wanted = settings.show_history && rest >= 3;
        let viz_fits = settings.show_viz && rest >= 5;
        let history_height = match (history_wanted, viz_fits) {
            (false, _) => 0,
            (true, false) => rest,
            // The visualizer has priority: history only when both still make sense.
            (true, true) if rest >= 12 => 7.min(rest - 5),
            (true, true) => 0,
        };
        let viz_height = if viz_fits { rest - history_height } else { 0 };
        let now_height = right.height - viz_height - history_height;
        let [viz, now, history] = Layout::vertical([
            Constraint::Length(viz_height),
            Constraint::Length(now_height),
            Constraint::Length(history_height),
        ])
        .areas(right);
        Panels {
            stations,
            info: show_info.then_some(info),
            viz: (viz_height > 0).then_some(viz),
            now,
            history: (history_height > 0).then_some(history),
            footer,
        }
    } else {
        let now_height = if body.height >= 7 { 4 } else { 3.min(body.height) };
        let viz_height = if settings.show_viz && body.height >= 18 { (body.height / 3).min(9) } else { 0 };
        let [viz, now, stations] = Layout::vertical([
            Constraint::Length(viz_height),
            Constraint::Length(now_height),
            Constraint::Min(0),
        ])
        .areas(body);
        Panels { stations, info: None, viz: (viz_height > 0).then_some(viz), now, history: None, footer }
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < 24 || area.height < 4 {
        frame.render_widget(Paragraph::new("terminal too small").style(fg(DIM)), area);
        return;
    }
    let panels = layout(area, app);
    draw_stations(frame, app, panels.stations);
    if let Some(rect) = panels.info {
        draw_info(frame, app, rect);
    }
    if let Some(rect) = panels.viz {
        draw_viz(frame, app, rect);
    }
    draw_now(frame, app, panels.now);
    if let Some(rect) = panels.history {
        draw_history(frame, app, rect);
    }
    if let Some(rect) = panels.footer {
        draw_footer(frame, app, rect);
    }
    if app.show_help {
        draw_help(frame, area);
    }
}

fn filter_header(app: &App, width: usize) -> Line<'static> {
    if app.searching || !app.search.is_empty() {
        let cursor = if app.searching { "▏" } else { "" };
        return Line::from(vec![Span::styled(" / ", bold(STAR)), Span::styled(format!("{}{cursor}", app.search), fg(BRIGHT))]);
    }
    let filters = app.filters();
    let current = filters.iter().position(|f| *f == app.settings.filter).unwrap_or(0);
    let mut spans = vec![Span::raw(" ")];
    let mut used = 1;
    for offset in 0..filters.len() {
        let name = &filters[(current + offset) % filters.len()];
        let label = if name == "favorites" { "★ favorites" } else { name.as_str() };
        let length = label.chars().count() + 2;
        if used + length > width {
            break;
        }
        used += length;
        spans.push(Span::styled(label.to_string(), if offset == 0 { bold(STAR) } else { fg(DIM) }));
        spans.push(Span::raw("  "));
    }
    Line::from(spans)
}

fn draw_stations(frame: &mut Frame, app: &mut App, area: Rect) {
    let order = if app.settings.sort_listeners { " by listeners" } else { "" };
    let block = panel("", "stations").title_bottom(Line::styled(format!("┤{} / {}{order}├", (app.selected + 1).min(app.visible.len()), app.visible.len()), fg(DIM)).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width < 4 {
        return;
    }
    let width = inner.width as usize;
    let mut lines = vec![filter_header(app, width)];
    let rows = (inner.height as usize).saturating_sub(1);

    if app.selected < app.offset {
        app.offset = app.selected;
    } else if rows > 0 && app.selected >= app.offset + rows {
        app.offset = app.selected + 1 - rows;
    }
    app.offset = app.offset.min(app.visible.len().saturating_sub(rows));

    if app.visible.is_empty() {
        let message = if app.channels.is_empty() {
            "loading stations…"
        } else if app.settings.filter == "favorites" && app.search.is_empty() {
            "no favorites yet: tab to all, f to add"
        } else {
            "no stations found"
        };
        lines.push(Line::styled(format!(" {message}"), fg(DIM)));
    }

    let name_width = width.saturating_sub(12);
    for (row, index) in app.visible.iter().enumerate().skip(app.offset).take(rows) {
        let channel = &app.channels[*index];
        let selected = row == app.selected;
        let favorite = app.is_favorite(&channel.id);
        let playing = app.playing.as_deref() == Some(channel.id.as_str());
        let background = if selected { Style::default().bg(SELECTED) } else { Style::default() };
        let name_color = if selected { BRIGHT } else if favorite { TEXT } else { DIM };
        let name_style = if selected { bold(name_color) } else { fg(name_color) };
        lines.push(
            Line::from(vec![
                Span::styled(if favorite { " ★ " } else { "   " }, fg(STAR)),
                Span::styled(if playing { "▶ " } else { "  " }, fg(TITLE)),
                Span::styled(pad(&channel.title, name_width), name_style),
                Span::styled(format!("{:>6} ", channel.listeners), fg(if selected { TITLE } else { DIM })),
            ])
            .style(background),
        );
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("²", "station");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(channel) = app.selected_channel() else { return };
    let description = Paragraph::new(Line::styled(channel.description.clone(), fg(TEXT))).wrap(Wrap { trim: true });
    let name = Line::styled(truncate(&channel.title, inner.width as usize), bold(BRIGHT));
    let meta = Line::from(vec![
        Span::styled(channel.genres().collect::<Vec<_>>().join(", "), fg(TITLE)),
        Span::styled(format!(" · {} listeners", channel.listeners), fg(DIM)),
    ]);
    let dj = Line::from(vec![Span::styled("DJ ", fg(DIM)), Span::styled(channel.dj.clone(), fg(TEXT))]);
    if inner.height >= 3 {
        let [head, top, bottom] = Layout::vertical([Constraint::Length(1), Constraint::Min(0), Constraint::Length(2)]).areas(inner);
        frame.render_widget(Paragraph::new(name), head);
        frame.render_widget(description, top);
        frame.render_widget(Paragraph::new(vec![dj, meta]), bottom);
    }
}

fn draw_viz(frame: &mut Frame, app: &mut App, area: Rect) {
    let title = if app.settings.viz_mode == "ship" { "nostalgia for infinity" } else { app.settings.viz_mode.as_str() };
    let block = panel("³", title).title(Line::from(vec![
        Span::styled("┤", fg(LINE)),
        Span::styled("v", bold(STAR)),
        Span::styled(" cycle ", fg(DIM)),
        Span::styled("h", bold(STAR)),
        Span::styled(" hide", fg(DIM)),
        Span::styled("├", fg(LINE)),
    ]).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let mode = app.settings.viz_mode.clone();
    app.visualizer.draw(&mode, &app.analyzer, frame.buffer_mut(), inner);
}

fn meter(label: &'static str, level: f32, width: usize) -> Vec<Span<'static>> {
    let filled = (level.clamp(0.0, 1.0) * width as f32).round() as usize;
    let mut spans = vec![Span::styled(format!("{label} "), fg(DIM))];
    for i in 0..width {
        let color = if i < filled { gradient(i as f32 / width.max(1) as f32) } else { EMPTY_METER };
        spans.push(Span::styled("■", fg(color)));
    }
    spans
}

fn current_track(app: &App) -> (String, String, String) {
    if app.songs_for == app.playing {
        if let Some(song) = app.songs.first() {
            return (song.title.clone(), song.artist.clone(), song.album.clone());
        }
    }
    let last = app.playing_channel().map(|c| c.last_playing.clone()).unwrap_or_default();
    match last.split_once(" - ") {
        Some((artist, title)) => (title.to_string(), artist.to_string(), String::new()),
        None => (last, String::new(), String::new()),
    }
}

fn draw_now(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("", "now playing");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let mut lines: Vec<Line> = Vec::new();

    let (symbol, color) = match &app.state {
        State::Playing => ("▶", TITLE),
        State::Connecting | State::Buffering => ("◌", STAR),
        State::Error(_) => ("!", ERROR),
        State::Stopped => ("■", DIM),
    };

    if let Some(channel) = app.playing_channel() {
        let (title, artist, album) = current_track(app);
        let shown = if title.is_empty() { channel.title.clone() } else { title };
        let mut first = vec![Span::styled(format!("{symbol} "), bold(color))];
        let on_air = app
            .songs
            .first()
            .filter(|_| app.songs_for == app.playing)
            .and_then(|song| song.timestamp())
            .map(|started| (App::now_timestamp() - started).max(0))
            .map(|s| format!("on air {:02}:{:02}", s / 60, s % 60));
        let clock_width = on_air.as_ref().map_or(0, |c| c.chars().count() + 1);
        first.push(Span::styled(pad(&shown, width.saturating_sub(2 + clock_width)), bold(BRIGHT)));
        if let Some(clock) = on_air {
            if width > 40 {
                first.push(Span::styled(format!(" {clock}"), fg(DIM)));
            }
        }
        lines.push(Line::from(first));

        let mut second = vec![Span::raw("  "), Span::styled(artist, fg(TITLE))];
        if !album.is_empty() {
            second.push(Span::styled(format!(" · {album}"), fg(DIM)));
        }
        second.push(Span::styled(format!(" · {}", channel.title), fg(DIM)));
        lines.push(Line::from(second));
    } else {
        lines.push(Line::from(vec![Span::styled(format!("{symbol} "), bold(color)), Span::styled("stopped: enter tunes in to a station", fg(TEXT))]));
        lines.push(Line::raw(""));
    }

    if let State::Error(message) = &app.state {
        lines.truncate(1);
        lines.push(Line::styled(format!("  {message}"), fg(ERROR)));
    }

    let volume_percent = (app.settings.volume * 100.0).round() as u32;
    let volume_text = if app.settings.muted { "mute".to_string() } else { format!("{volume_percent}%") };
    let side = width >= 52;
    let meter_width = if side { width.saturating_sub(28).min(40) } else { width.saturating_sub(3) };
    let slider_width = 12usize;
    let filled = (app.settings.volume * slider_width as f32).round() as usize;

    let mut left = meter("L", app.analyzer.level_l, meter_width);
    let mut right = meter("R", app.analyzer.level_r, meter_width);
    if side {
        left.push(Span::styled("  vol ", fg(DIM)));
        let color = if app.settings.muted { DIM } else { TITLE };
        left.push(Span::styled("━".repeat(filled), fg(color)));
        left.push(Span::styled("─".repeat(slider_width - filled), fg(LINE)));
        left.push(Span::styled(format!(" {volume_text}"), fg(TEXT)));

        right.push(Span::styled("  buf ", fg(DIM)));
        let buffer_cells = ((app.buffered / 6.0).clamp(0.0, 1.0) * slider_width as f32).round() as usize;
        right.push(Span::styled("█".repeat(buffer_cells), fg(TITLE)));
        right.push(Span::styled("░".repeat(slider_width - buffer_cells), fg(LINE)));
        right.push(Span::styled(format!(" {:.1}s", app.buffered), fg(TEXT)));
    }
    lines.push(Line::from(left));
    lines.push(Line::from(right));

    let status = match &app.state {
        State::Connecting => "connecting…".to_string(),
        State::Buffering => format!("buffering… {}", app.server),
        _ => app.server.clone(),
    };
    let mut bottom = vec![Span::styled(format!("♪ {status}"), fg(DIM))];
    bottom.push(Span::styled(format!("  ·  {}", QUALITIES[app.settings.quality].0), fg(DIM)));
    if !side {
        bottom.push(Span::styled(format!("  ·  vol {volume_text}"), fg(DIM)));
    }
    if let Some(channel) = app.playing_channel() {
        bottom.push(Span::styled(format!("  ·  {} listeners", channel.listeners), fg(DIM)));
    }
    lines.push(Line::from(bottom));

    // Short panels keep the track; a compact volume line replaces the meters.
    if inner.height < 5 && inner.height >= 2 {
        lines.truncate(2.min(lines.len()));
        if inner.height >= 3 {
            lines.push(Line::styled(format!("  vol {volume_text}  ·  {}", QUALITIES[app.settings.quality].0), fg(DIM)));
        }
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_history(frame: &mut Frame, app: &App, area: Rect) {
    let block = panel("⁵", "recently played");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    if app.songs_for != app.focus_id() {
        frame.render_widget(Paragraph::new(Line::styled("loading…", fg(DIM))), inner);
        return;
    }
    let artist_width = (width.saturating_sub(7) * 2 / 5).max(8);
    let title_width = width.saturating_sub(7 + artist_width + 1);
    let skip = usize::from(app.playing.is_some());
    let lines: Vec<Line> = app
        .songs
        .iter()
        .skip(skip)
        .take(inner.height as usize)
        .map(|song| {
            let time = song.timestamp().map(App::clock_time).unwrap_or_default();
            Line::from(vec![
                Span::styled(format!("{time:<6} "), fg(DIM)),
                Span::styled(pad(&song.artist, artist_width), fg(TITLE)),
                Span::raw(" "),
                Span::styled(truncate(&song.title, title_width), fg(TEXT)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    if let Some(error) = app.notice.as_ref().or(app.network_error.as_ref()) {
        frame.render_widget(Paragraph::new(Line::styled(format!(" {error}"), fg(ERROR))), area);
        return;
    }
    // In screen order; the number is the priority, lowest goes first when space is short.
    let keys: &[(&str, &str, u8)] = if app.searching {
        &[("type", "to search", 0), ("enter", "keep", 1), ("esc", "clear", 2)]
    } else {
        &[("space", "play", 2), ("↑↓", "select", 11), ("enter", "tune", 3), ("f", "fav", 4), ("tab", "filter", 8), ("n/p", "next/prev", 7), ("q", "quality", 10), ("v", "viz", 6), ("h", "hide", 5), ("/", "search", 9), ("?", "help", 1), ("esc", "quit", 0)]
    };
    let cost = |key: &str, label: &str| key.chars().count() + label.chars().count() + 3;
    let mut by_priority: Vec<_> = keys.iter().collect();
    by_priority.sort_by_key(|k| k.2);
    let mut room = (area.width as usize).saturating_sub(1);
    let mut shown: Vec<&str> = Vec::new();
    for (key, label, _) in by_priority {
        if cost(key, label) <= room {
            room -= cost(key, label);
            shown.push(key);
        }
    }
    let mut spans = vec![Span::raw(" ")];
    for (key, label, _) in keys.iter().filter(|k| shown.contains(&k.0)) {
        spans.push(Span::styled(key.to_string(), bold(STAR)));
        spans.push(Span::styled(format!(" {label}  "), fg(DIM)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let rows: &[(&str, &str)] = &[
        ("space", "play or stop"),
        ("enter", "tune in to the selected station"),
        ("↑ ↓ j k", "select, PgUp/PgDn and g/G to jump"),
        ("n p", "next or previous favorite"),
        ("f", "add or remove favorite"),
        ("tab", "favorites, all, or a genre"),
        ("/", "search, esc clears it"),
        ("s", "sort by name or listeners"),
        ("q", "stream quality"),
        ("← → + -", "volume"),
        ("m", "mute"),
        ("v", "next visualizer"),
        ("h or 3", "hide or show the visualizer"),
        ("2 5", "hide or show station info, history"),
        ("r", "reload stations"),
        ("esc", "quit"),
    ];
    let width = 46.min(area.width);
    let height = (rows.len() as u16 + 2).min(area.height);
    let rect = Rect::new(area.x + (area.width - width) / 2, area.y + (area.height - height) / 2, width, height);
    frame.render_widget(Clear, rect);
    let lines: Vec<Line> = rows
        .iter()
        .map(|(key, text)| Line::from(vec![Span::styled(format!(" {key:<9}"), bold(STAR)), Span::styled(text.to_string(), fg(TEXT))]))
        .collect();
    let block = panel("", "help").title_alignment(Alignment::Left);
    frame.render_widget(Paragraph::new(lines).block(block), rect);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Channel, Song};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn test_app() -> App {
        let channels = (0..30)
            .map(|i| Channel {
                id: if i == 0 { "a".into() } else { format!("s{i}") },
                title: format!("A station with a rather long name {i}"),
                description: "Celebrating NASA and space explorers everywhere, in a long sentence that has to wrap.".into(),
                dj: "Rusty Hodge".into(),
                genre: "ambient|news".into(),
                listeners: "412".into(),
                last_playing: "Brian Eno - An Ending (Ascent)".into(),
                ..Default::default()
            })
            .collect();
        let mut app = App::for_tests(channels);
        app.settings.filter = "all".into();
        app.rebuild(Some("a".into()));
        app.playing = Some("a".into());
        app.songs_for = Some("a".into());
        app.songs = (0..9)
            .map(|i| Song { title: format!("Track number {i} with a long title"), artist: "Brian Eno".into(), album: "Apollo".into(), date: "1791634564".into() })
            .collect();
        app
    }

    fn render(app: &mut App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = String::new();
        for y in 0..height {
            for x in 0..width {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn never_panics_at_any_size_or_mode() {
        for mode in crate::viz::MODES {
            for show_viz in [true, false] {
                let mut app = test_app();
                app.settings.viz_mode = mode.to_string();
                app.settings.show_viz = show_viz;
                for width in (1..130).step_by(5) {
                    for height in (1..50).step_by(2) {
                        render(&mut app, width, height);
                    }
                }
                app.show_help = true;
                for width in (1..130).step_by(7) {
                    for height in (1..50).step_by(3) {
                        render(&mut app, width, height);
                    }
                }
            }
        }
    }

    #[test]
    fn the_full_layout_shows_every_panel() {
        let mut app = test_app();
        let out = render(&mut app, 96, 28);
        for text in ["stations", "station", "nostalgia for infinity", "now playing", "recently played", "esc quit"] {
            assert!(out.contains(text), "missing {text} in\n{out}");
        }
    }

    #[test]
    fn hiding_the_visualizer_gives_its_room_to_the_history() {
        let mut app = test_app();
        app.settings.show_viz = false;
        let out = render(&mut app, 96, 28);
        assert!(!out.contains("nostalgia for infinity"), "{out}");
        assert!(out.contains("recently played"));
        assert!(out.contains("Track number 8"), "history should use the free room\n{out}");
    }

    #[test]
    fn small_terminals_drop_panels_but_keep_list_and_track() {
        let mut app = test_app();
        let out = render(&mut app, 50, 12);
        assert!(!out.contains("nostalgia"), "{out}");
        assert!(out.contains("stations") && out.contains("now playing"), "{out}");
        assert!(out.contains("Track number 0"), "{out}");
        assert!(render(&mut app, 10, 2).contains("terminal"));
    }
}
