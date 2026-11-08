//! Plays a stream: ffmpeg decodes it to raw samples, we send those to the sound
//! card and keep the samples that are being played for the visualizer. That way
//! the visualizer moves on what you hear without tapping the system audio.

use crate::api;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const PREBUFFER_SECONDS: f32 = 1.0;
const MAX_BUFFER_SECONDS: f32 = 6.0;
const ANALYSIS_FRAMES: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub enum State {
    Stopped,
    Connecting,
    Buffering,
    Playing,
    Error(String),
}

struct Shared {
    samples: Mutex<VecDeque<f32>>,
    analysis: Mutex<VecDeque<(f32, f32)>>,
    gain: AtomicU32,
    primed: AtomicBool,
    generation: AtomicU64,
    state: Mutex<State>,
    server: Mutex<String>,
    rate: u32,
}

impl Shared {
    fn set_state(&self, generation: u64, state: State) {
        if self.generation.load(Ordering::SeqCst) == generation {
            *self.state.lock().unwrap() = state;
        }
    }
}

pub struct Snapshot {
    pub state: State,
    pub server: String,
    pub buffered: f32,
}

pub struct Audio {
    shared: Arc<Shared>,
    _stream: cpal::Stream,
}

fn write_output<T>(out: &mut [T], channels: usize, shared: &Shared)
where
    T: SizedSample + FromSample<f32>,
{
    let gain = f32::from_bits(shared.gain.load(Ordering::Relaxed));
    let mut samples = shared.samples.lock().unwrap();
    if !shared.primed.load(Ordering::Relaxed) {
        if samples.len() < (shared.rate as f32 * 2.0 * PREBUFFER_SECONDS) as usize {
            out.fill(T::from_sample(0.0));
            return;
        }
        shared.primed.store(true, Ordering::Relaxed);
    }
    let mut analysis = shared.analysis.try_lock().ok();
    for frame in out.chunks_mut(channels) {
        let (l, r) = match (samples.pop_front(), samples.pop_front()) {
            (Some(l), Some(r)) => (l, r),
            _ => {
                shared.primed.store(false, Ordering::Relaxed);
                (0.0, 0.0)
            }
        };
        if let Some(ring) = analysis.as_mut() {
            if ring.len() >= ANALYSIS_FRAMES {
                ring.pop_front();
            }
            ring.push_back((l, r));
        }
        if channels == 1 {
            frame[0] = T::from_sample((l + r) * 0.5 * gain);
        } else {
            for (i, slot) in frame.iter_mut().enumerate() {
                *slot = T::from_sample(if i % 2 == 0 { l } else { r } * gain);
            }
        }
    }
}

fn build_stream<T>(device: &cpal::Device, config: &cpal::StreamConfig, shared: Arc<Shared>) -> Result<cpal::Stream, String>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    device
        .build_output_stream(config, move |out: &mut [T], _| write_output(out, channels, &shared), |_| {}, None)
        .map_err(|e| e.to_string())
}

impl Audio {
    pub fn new() -> Result<Audio, String> {
        let device = cpal::default_host().default_output_device().ok_or("no audio output device found")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let shared = Arc::new(Shared {
            samples: Mutex::new(VecDeque::new()),
            analysis: Mutex::new(VecDeque::with_capacity(ANALYSIS_FRAMES)),
            gain: AtomicU32::new(0f32.to_bits()),
            primed: AtomicBool::new(false),
            generation: AtomicU64::new(0),
            state: Mutex::new(State::Stopped),
            server: Mutex::new(String::new()),
            rate: config.sample_rate.0,
        });
        let stream = match format {
            SampleFormat::F32 => build_stream::<f32>(&device, &config, shared.clone()),
            SampleFormat::I16 => build_stream::<i16>(&device, &config, shared.clone()),
            SampleFormat::U16 => build_stream::<u16>(&device, &config, shared.clone()),
            SampleFormat::I32 => build_stream::<i32>(&device, &config, shared.clone()),
            other => Err(format!("unsupported sample format {other}")),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Audio { shared, _stream: stream })
    }

    pub fn sample_rate(&self) -> u32 {
        self.shared.rate
    }

    /// `volume` is what the slider shows; squaring it makes the steps sound even.
    pub fn set_volume(&self, volume: f32, muted: bool) {
        let gain = if muted { 0.0 } else { volume * volume };
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn play(&self, playlist_url: String) {
        let generation = self.reset(State::Connecting);
        let shared = self.shared.clone();
        thread::spawn(move || feed(shared, generation, playlist_url));
    }

    pub fn stop(&self) {
        self.reset(State::Stopped);
    }

    fn reset(&self, state: State) -> u64 {
        let generation = self.shared.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.shared.samples.lock().unwrap().clear();
        self.shared.analysis.lock().unwrap().clear();
        self.shared.primed.store(false, Ordering::Relaxed);
        *self.shared.state.lock().unwrap() = state;
        self.shared.server.lock().unwrap().clear();
        generation
    }

    pub fn snapshot(&self) -> Snapshot {
        let primed = self.shared.primed.load(Ordering::Relaxed);
        let mut state = self.shared.state.lock().unwrap().clone();
        if state == State::Playing && !primed {
            state = State::Buffering;
        }
        let buffered = self.shared.samples.lock().unwrap().len() as f32 / (self.shared.rate as f32 * 2.0);
        Snapshot { state, server: self.shared.server.lock().unwrap().clone(), buffered }
    }

    pub fn copy_analysis(&self, out: &mut Vec<(f32, f32)>) {
        out.clear();
        out.extend(self.shared.analysis.lock().unwrap().iter().copied());
    }
}

struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_ffmpeg(url: &str, rate: u32) -> std::io::Result<KillOnDrop> {
    let child = Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "quiet", "-user_agent", api::USER_AGENT])
        .args(["-reconnect", "1", "-reconnect_streamed", "1", "-reconnect_delay_max", "5"])
        .args(["-i", url, "-vn", "-ac", "2", "-ar", &rate.to_string(), "-f", "f32le", "-acodec", "pcm_f32le", "pipe:1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(KillOnDrop(child))
}

fn host_of(url: &str) -> String {
    url.split("//").nth(1).unwrap_or(url).split('/').next().unwrap_or(url).to_string()
}

/// Reads one stream until it ends. Returns whether it delivered any audio.
fn pump(shared: &Shared, generation: u64, url: &str) -> Result<bool, String> {
    let mut ffmpeg = spawn_ffmpeg(url, shared.rate).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => "ffmpeg is not installed".to_string(),
        _ => e.to_string(),
    })?;
    let mut stdout = ffmpeg.0.stdout.take().ok_or("could not read from ffmpeg")?;
    *shared.server.lock().unwrap() = host_of(url);

    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
    thread::spawn(move || {
        let mut chunk = vec![0u8; 8192];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(chunk[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let limit = (shared.rate as f32 * 2.0 * MAX_BUFFER_SECONDS) as usize;
    let mut carry: Vec<u8> = Vec::new();
    let mut got_audio = false;
    let started = Instant::now();
    loop {
        if shared.generation.load(Ordering::SeqCst) != generation {
            return Ok(got_audio);
        }
        if !got_audio && started.elapsed() > Duration::from_secs(20) {
            return Ok(false);
        }
        if shared.samples.lock().unwrap().len() >= limit {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(bytes) => {
                carry.extend_from_slice(&bytes);
                let whole = carry.len() / 4 * 4;
                let mut samples = shared.samples.lock().unwrap();
                if shared.generation.load(Ordering::SeqCst) != generation {
                    return Ok(got_audio);
                }
                samples.extend(carry[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                drop(samples);
                carry.drain(..whole);
                if !got_audio {
                    got_audio = true;
                    shared.set_state(generation, State::Playing);
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(got_audio),
        }
    }
}

fn feed(shared: Arc<Shared>, generation: u64, playlist_url: String) {
    let streams = match api::fetch_streams(&playlist_url) {
        Ok(streams) => streams,
        Err(e) => return shared.set_state(generation, State::Error(format!("could not load the stream: {e}"))),
    };
    let mut index = 0;
    let mut failures = 0;
    while shared.generation.load(Ordering::SeqCst) == generation {
        match pump(&shared, generation, &streams[index % streams.len()]) {
            Err(message) => return shared.set_state(generation, State::Error(message)),
            Ok(true) => failures = 0,
            Ok(false) => failures += 1,
        }
        if shared.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        if failures >= streams.len() {
            return shared.set_state(generation, State::Error("no server is delivering this stream".into()));
        }
        // A live stream should not end: the server dropped us, so try the next mirror.
        shared.set_state(generation, State::Connecting);
        index += 1;
        thread::sleep(Duration::from_millis(1000));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_host_of_a_stream() {
        assert_eq!(host_of("https://ice2.somafm.com/groovesalad-128-aac"), "ice2.somafm.com");
    }
}
