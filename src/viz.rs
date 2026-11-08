//! Turns the samples being played into levels and draws the visualizers.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use rustfft::{num_complex::Complex, Fft, FftPlanner};
use std::sync::Arc;

const FFT_SIZE: usize = 2048;
pub const BANDS: usize = 64;
const MIN_HZ: f32 = 40.0;
const MAX_HZ: f32 = 16000.0;
const RANGE_DB: f32 = 50.0;

pub const MODES: [&str; 4] = ["ship", "spectrum", "mirror", "wave"];

pub struct Analyzer {
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    scratch: Vec<Complex<f32>>,
    pub bands: [f32; BANDS],
    pub peaks: [f32; BANDS],
    pub wave: Vec<f32>,
    pub level_l: f32,
    pub level_r: f32,
    pub bass: f32,
    reference_db: f32,
}

fn to_db(v: f32) -> f32 {
    20.0 * v.max(1e-7).log10()
}

impl Analyzer {
    pub fn new() -> Analyzer {
        let window = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_SIZE - 1) as f32).cos())
            .collect();
        Analyzer {
            fft: FftPlanner::new().plan_fft_forward(FFT_SIZE),
            window,
            scratch: vec![Complex::new(0.0, 0.0); FFT_SIZE],
            bands: [0.0; BANDS],
            peaks: [0.0; BANDS],
            wave: Vec::new(),
            level_l: 0.0,
            level_r: 0.0,
            bass: 0.0,
            reference_db: -30.0,
        }
    }

    /// `samples` holds the most recently played stereo frames; `dt` is in seconds.
    pub fn update(&mut self, samples: &[(f32, f32)], rate: u32, dt: f32) {
        let mut targets = [0f32; BANDS];
        let (mut target_l, mut target_r) = (0.0, 0.0);
        self.wave.clear();

        if samples.len() >= FFT_SIZE {
            let recent = &samples[samples.len() - FFT_SIZE..];
            for (i, (l, r)) in recent.iter().enumerate() {
                self.scratch[i] = Complex::new((l + r) * 0.5 * self.window[i], 0.0);
            }
            self.fft.process(&mut self.scratch);

            let mut band_db = [f32::MIN; BANDS];
            let scale = 4.0 / FFT_SIZE as f32;
            let bin_hz = rate as f32 / FFT_SIZE as f32;
            for (band, slot) in band_db.iter_mut().enumerate() {
                let (lo, hi) = band_range(band);
                let first = ((lo / bin_hz) as usize).clamp(1, FFT_SIZE / 2 - 1);
                let last = ((hi / bin_hz).ceil() as usize).clamp(first + 1, FFT_SIZE / 2);
                let peak = self.scratch[first..last].iter().map(|c| c.norm()).fold(0.0, f32::max);
                // Music has far less energy up high; tilt so the treble is visible.
                *slot = to_db(peak * scale) + 3.0 * (band as f32 / BANDS as f32 * 7.0);
            }

            // Follow the loudest band slowly, so quiet ambient still fills the panel.
            let loudest = band_db.iter().copied().fold(f32::MIN, f32::max).max(-70.0);
            let fall = 3.0 * dt;
            self.reference_db = if loudest > self.reference_db { loudest } else { (self.reference_db - fall).max(loudest) };
            for (target, db) in targets.iter_mut().zip(band_db) {
                *target = ((db - (self.reference_db - RANGE_DB)) / RANGE_DB).clamp(0.0, 1.0);
            }

            let rms = |pick: fn(&(f32, f32)) -> f32| (recent.iter().map(|s| pick(s).powi(2)).sum::<f32>() / recent.len() as f32).sqrt();
            target_l = ((to_db(rms(|s| s.0)) + 45.0) / 45.0).clamp(0.0, 1.0);
            target_r = ((to_db(rms(|s| s.1)) + 45.0) / 45.0).clamp(0.0, 1.0);

            let peak = recent.iter().map(|(l, r)| (l + r).abs() * 0.5).fold(0.02, f32::max);
            self.wave.extend(recent.iter().map(|(l, r)| (l + r) * 0.5 / peak));
        }

        let attack = 1.0 - (-dt * 28.0).exp();
        let release = 1.0 - (-dt * 7.0).exp();
        let ease = |current: &mut f32, target: f32| {
            let k = if target > *current { attack } else { release };
            *current += (target - *current) * k;
        };
        for i in 0..BANDS {
            ease(&mut self.bands[i], targets[i]);
            self.peaks[i] = (self.peaks[i] - 0.35 * dt).max(self.bands[i]);
        }
        ease(&mut self.level_l, target_l);
        ease(&mut self.level_r, target_r);
        let low = BANDS / 5;
        let bass_target = targets[..low].iter().sum::<f32>() / low as f32;
        ease(&mut self.bass, bass_target * bass_target);
    }

    fn band_at(values: &[f32; BANDS], position: f32) -> f32 {
        let x = position.clamp(0.0, 1.0) * (BANDS - 1) as f32;
        let i = x as usize;
        let next = (i + 1).min(BANDS - 1);
        values[i] + (values[next] - values[i]) * (x - i as f32)
    }
}

fn band_range(band: usize) -> (f32, f32) {
    let ratio = MAX_HZ / MIN_HZ;
    let at = |i: usize| MIN_HZ * ratio.powf(i as f32 / BANDS as f32);
    (at(band), at(band + 1))
}

const STOPS: [(f32, (u8, u8, u8)); 5] = [
    (0.0, (45, 212, 191)),
    (0.35, (56, 189, 248)),
    (0.65, (129, 140, 248)),
    (0.85, (232, 121, 249)),
    (1.0, (255, 95, 126)),
];

pub fn gradient(p: f32) -> Color {
    let p = p.clamp(0.0, 1.0);
    for pair in STOPS.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if p <= b {
            let t = (p - a) / (b - a);
            let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
            return Color::Rgb(mix(ca.0, cb.0), mix(ca.1, cb.1), mix(ca.2, cb.2));
        }
    }
    Color::Rgb(255, 95, 126)
}

const EIGHTHS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const WHITE: Color = Color::Rgb(255, 255, 255);
const HULL: Color = Color::Rgb(200, 211, 224);
const LINE: Color = Color::Rgb(59, 74, 99);
const DIM: Color = Color::Rgb(92, 107, 130);
const MID: Color = Color::Rgb(138, 160, 191);

fn put(buf: &mut Buffer, area: Rect, x: i32, y: i32, text: &str, color: Color) {
    for (i, ch) in text.chars().enumerate() {
        let cx = x + i as i32;
        if cx < 0 || y < 0 || cx >= area.width as i32 || y >= area.height as i32 {
            continue;
        }
        if let Some(cell) = buf.cell_mut((area.x + cx as u16, area.y + y as u16)) {
            cell.set_char(ch);
            cell.set_fg(color);
        }
    }
}

struct Star {
    x: f32,
    y: f32,
    speed: f32,
}

pub struct Visualizer {
    stars: Vec<Star>,
    seed: u32,
    time: f32,
}

impl Visualizer {
    pub fn new() -> Visualizer {
        let mut viz = Visualizer { stars: Vec::new(), seed: 0x2F6E_2B1D, time: 0.0 };
        for _ in 0..70 {
            let star = Star { x: viz.random(), y: viz.random(), speed: 0.3 + viz.random() * 1.4 };
            viz.stars.push(star);
        }
        viz
    }

    fn random(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        (self.seed % 10_000) as f32 / 10_000.0
    }

    pub fn advance(&mut self, analyzer: &Analyzer, dt: f32) {
        self.time += dt;
        // Stars keep drifting when it is silent; the bass pushes the ship faster.
        let velocity = 0.012 + analyzer.bass * 0.16;
        for i in 0..self.stars.len() {
            self.stars[i].x -= self.stars[i].speed * velocity * dt * 5.0;
            if self.stars[i].x < 0.0 {
                self.stars[i].x = 1.0;
                self.stars[i].y = self.random();
                self.stars[i].speed = 0.3 + self.random() * 1.4;
            }
        }
    }

    pub fn draw(&self, mode: &str, analyzer: &Analyzer, buf: &mut Buffer, area: Rect) {
        match mode {
            "spectrum" => draw_spectrum(analyzer, buf, area),
            "mirror" => draw_mirror(analyzer, buf, area),
            "wave" => draw_wave(analyzer, buf, area),
            _ => self.draw_ship(analyzer, buf, area),
        }
    }

    fn draw_ship(&self, analyzer: &Analyzer, buf: &mut Buffer, area: Rect) {
        let (w, h) = (area.width as i32, area.height as i32);
        if w < 12 || h < 3 {
            return;
        }
        let bass = analyzer.bass;
        // About one star per 14 cells, whatever the size of the panel.
        let count = ((w * h / 14) as usize).clamp(4, self.stars.len());
        for star in &self.stars[..count] {
            let x = (star.x * w as f32) as i32;
            let y = (star.y * h as f32) as i32;
            let color = if star.speed > 1.2 { WHITE } else if star.speed > 0.7 { MID } else { DIM };
            let trail = (star.speed * bass * 5.0) as i32;
            for i in 1..=trail.min(6) {
                put(buf, area, x + i, y, "─", LINE);
            }
            put(buf, area, x, y, if star.speed > 1.45 { "✦" } else { "·" }, color);
        }

        let bob = ((self.time * 0.8).sin() * 0.6).round() as i32;
        let top = (h / 2 - 1 + bob).clamp(0, h - 3);
        let x = w / 3;
        let length = 3 + (bass * (x as f32 - 4.0).max(4.0)) as i32;
        for i in 0..length {
            let p = i as f32 / length as f32;
            let color = if p < 0.4 { Color::Rgb(224, 247, 255) } else if p < 0.75 { Color::Rgb(56, 189, 248) } else { Color::Rgb(129, 140, 248) };
            put(buf, area, x - 3 - i, top, if i % 2 == 1 { "═" } else { "≈" }, color);
            put(buf, area, x - 3 - i, top + 2, if i % 2 == 1 { "≈" } else { "═" }, color);
        }
        put(buf, area, x - 2, top, "▐██▌", HULL);
        put(buf, area, x - 2, top + 2, "▐██▌", HULL);
        put(buf, area, x - 2, top + 1, "╺━", DIM);
        put(buf, area, x, top + 1, "▐███▙▄▄▄▄▄▄▄▄▄▄▄▄▄▖▁▁▁", WHITE);

        // Radio waves leave the bow, one arc further for every step in level.
        let level = analyzer.level_l.max(analyzer.level_r);
        let arcs = (level * 4.0) as i32;
        for i in 0..arcs.min(3) {
            put(buf, area, x + 23 + i * 2, top + 1, ")", gradient(0.2 + i as f32 * 0.3));
        }
    }
}

fn draw_spectrum(analyzer: &Analyzer, buf: &mut Buffer, area: Rect) {
    let (w, h) = (area.width as i32, area.height as i32);
    let columns = w / 2;
    for i in 0..columns {
        let position = i as f32 / (columns - 1).max(1) as f32;
        let value = Analyzer::band_at(&analyzer.bands, position) * h as f32;
        let peak = (Analyzer::band_at(&analyzer.peaks, position) * h as f32) as i32;
        for y in 0..h {
            let row = h - 1 - y;
            let fill = value - y as f32;
            if fill >= 1.0 || fill > 0.0 {
                let ch = if fill >= 1.0 { '█' } else { EIGHTHS[(fill * 8.0) as usize] };
                let text: String = [ch, ch].iter().collect();
                put(buf, area, i * 2, row, &text, gradient(y as f32 / (h - 1).max(1) as f32));
            } else if y == peak.min(h - 1) && peak > 0 {
                put(buf, area, i * 2, row, "▔▔", WHITE);
            }
        }
    }
}

fn draw_mirror(analyzer: &Analyzer, buf: &mut Buffer, area: Rect) {
    let (w, h) = (area.width as i32, area.height as i32);
    let columns = w / 2;
    let middle = h / 2;
    for i in 0..columns {
        let position = i as f32 / (columns - 1).max(1) as f32;
        let value = Analyzer::band_at(&analyzer.bands, position) * middle.max(1) as f32;
        put(buf, area, i * 2, middle, "──", LINE);
        for y in 0..middle {
            let color = gradient(y as f32 / (middle - 1).max(1) as f32);
            let fill = value - y as f32;
            if fill <= 0.0 {
                break;
            }
            let up = if fill >= 1.0 { '█' } else { EIGHTHS[(fill * 8.0) as usize] };
            let down = if fill >= 1.0 { '█' } else if fill >= 0.5 { '▀' } else { ' ' };
            put(buf, area, i * 2, middle - 1 - y, &[up, up].iter().collect::<String>(), color);
            put(buf, area, i * 2, middle + 1 + y, &[down, down].iter().collect::<String>(), color);
        }
    }
}

fn draw_wave(analyzer: &Analyzer, buf: &mut Buffer, area: Rect) {
    let (w, h) = (area.width as i32, area.height as i32);
    let middle = (h - 1) as f32 / 2.0;
    for x in 0..w {
        put(buf, area, x, middle.round() as i32, "·", LINE);
    }
    if analyzer.wave.is_empty() {
        return;
    }
    let level = analyzer.level_l.max(analyzer.level_r);
    for x in 0..w {
        // Average per column: picking single samples looks like noise.
        let len = analyzer.wave.len();
        let bucket = &analyzer.wave[x as usize * len / w as usize..((x as usize + 1) * len / w as usize).max(x as usize * len / w as usize + 1).min(len)];
        let sample = bucket.iter().sum::<f32>() / bucket.len() as f32 * 1.6;
        let offset = sample * middle * level.max(0.2);
        let y = (middle - offset).clamp(0.0, (h - 1) as f32);
        let glyph = if y - y.floor() >= 0.5 { "▄" } else { "▀" };
        put(buf, area, x, y as i32, glyph, gradient(offset.abs() / middle.max(1.0)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, amplitude: f32, rate: u32) -> Vec<(f32, f32)> {
        (0..4096)
            .map(|i| {
                let v = (i as f32 / rate as f32 * hz * 2.0 * std::f32::consts::PI).sin() * amplitude;
                (v, v)
            })
            .collect()
    }

    fn settle(analyzer: &mut Analyzer, samples: &[(f32, f32)]) {
        for _ in 0..30 {
            analyzer.update(samples, 48000, 0.04);
        }
    }

    #[test]
    fn a_tone_lights_up_its_own_band() {
        for hz in [80.0, 1000.0, 8000.0] {
            let mut analyzer = Analyzer::new();
            settle(&mut analyzer, &sine(hz, 0.3, 48000));
            let top = (0..BANDS).max_by(|a, b| analyzer.bands[*a].total_cmp(&analyzer.bands[*b])).unwrap();
            let (lo, hi) = band_range(top);
            assert!(lo * 0.7 <= hz && hz <= hi * 1.4, "{hz} Hz peaked in band {top} ({lo}-{hi})");
        }
    }

    #[test]
    fn bass_follows_low_notes_only() {
        let mut low = Analyzer::new();
        settle(&mut low, &sine(70.0, 0.4, 48000));
        let mut high = Analyzer::new();
        settle(&mut high, &sine(9000.0, 0.4, 48000));
        assert!(low.bass > 0.1, "{}", low.bass);
        assert!(low.bass > high.bass * 2.0, "{} vs {}", low.bass, high.bass);
    }

    #[test]
    fn silence_decays_to_nothing() {
        let mut analyzer = Analyzer::new();
        settle(&mut analyzer, &sine(200.0, 0.5, 48000));
        assert!(analyzer.level_l > 0.5);
        for _ in 0..100 {
            analyzer.update(&[], 48000, 0.04);
        }
        assert!(analyzer.level_l < 0.01 && analyzer.bass < 0.01);
        assert!(analyzer.bands.iter().all(|b| *b < 0.01));
    }

    #[test]
    fn louder_means_a_higher_meter() {
        let mut quiet = Analyzer::new();
        settle(&mut quiet, &sine(440.0, 0.02, 48000));
        let mut loud = Analyzer::new();
        settle(&mut loud, &sine(440.0, 0.5, 48000));
        assert!(loud.level_l > quiet.level_l + 0.2, "{} vs {}", loud.level_l, quiet.level_l);
    }
}
