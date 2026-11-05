//! WASAPI loopback capture + spectrum analysis.

use crate::viz::Frame;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use realfft::RealFftPlanner;
use std::sync::{Arc, Mutex};
use std::time::Instant;

const N: usize = 4096;
const BANDS: usize = 64;

struct Shared {
    buf: Vec<f32>,
    last_sound: Option<Instant>,
}

pub struct Audio {
    _stream: cpal::Stream,
    shared: Arc<Mutex<Shared>>,
    fft: Arc<dyn realfft::RealToComplex<f32>>,
    window: Vec<f32>,
    edges: Vec<usize>,
    level: [f32; BANDS],
    peak: [f32; 32],
    peak_t: [Option<Instant>; 32],
    gain: f32,
    wmax: f32,
    bass_avg: f32,
    last_beat: Option<Instant>,
}

/// Output devices, for the settings window.
pub fn output_devices() -> Vec<String> {
    cpal::host_from_id(cpal::HostId::Wasapi)
        .ok()
        .and_then(|h| h.output_devices().ok())
        .map(|ds| ds.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

impl Audio {
    /// `matching`: part of the output device name to capture ("" = default output).
    pub fn new(matching: &str) -> anyhow::Result<Audio> {
        let host = cpal::host_from_id(cpal::HostId::Wasapi)?;
        let mut device = None;
        if !matching.is_empty() {
            let m = matching.to_lowercase();
            device = host.output_devices()?.find(|d| d.name().is_ok_and(|n| n.to_lowercase().contains(&m)));
        }
        let device = match device {
            Some(d) => d,
            None => host.default_output_device().ok_or_else(|| anyhow::anyhow!("no output device"))?,
        };
        let cfg = device.default_output_config()?;
        let rate = cfg.sample_rate().0 as f32;
        let ch = cfg.channels() as usize;
        let shared = Arc::new(Mutex::new(Shared { buf: vec![0.0; N], last_sound: None }));
        let sh = shared.clone();
        let push = move |mono: &[f32]| {
            let mut s = sh.lock().unwrap();
            if mono.iter().any(|v| v.abs() > 1e-4) {
                s.last_sound = Some(Instant::now());
            }
            let n = mono.len().min(N);
            s.buf.drain(..n);
            s.buf.extend_from_slice(&mono[mono.len() - n..]);
        };
        let err = |e| eprintln!("audio stream error: {e}");
        // building an *input* stream on an output device = WASAPI loopback
        let stream = match cfg.sample_format() {
            cpal::SampleFormat::F32 => device.build_input_stream(
                &cfg.config(),
                move |data: &[f32], _| {
                    let mono: Vec<f32> = data.chunks(ch).map(|c| c.iter().sum::<f32>() / ch as f32).collect();
                    push(&mono)
                },
                err,
                None,
            )?,
            cpal::SampleFormat::I16 => device.build_input_stream(
                &cfg.config(),
                move |data: &[i16], _| {
                    let mono: Vec<f32> =
                        data.chunks(ch).map(|c| c.iter().map(|v| *v as f32 / 32768.0).sum::<f32>() / ch as f32).collect();
                    push(&mono)
                },
                err,
                None,
            )?,
            f => anyhow::bail!("unsupported sample format {f:?}"),
        };
        stream.play()?;

        // log-spaced band edges as FFT bin indices; every band gets at least one bin
        let mut edges: Vec<usize> = (0..=BANDS)
            .map(|i| (40.0f32 * (16000.0f32 / 40.0).powf(i as f32 / BANDS as f32) * N as f32 / rate).round() as usize)
            .collect();
        for i in 1..edges.len() {
            edges[i] = edges[i].max(edges[i - 1] + 1);
        }
        let window = (0..N).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos()).collect();
        Ok(Audio {
            _stream: stream,
            shared,
            fft: RealFftPlanner::<f32>::new().plan_fft_forward(N),
            window,
            edges,
            level: [0.0; BANDS],
            peak: [0.0; 32],
            peak_t: [None; 32],
            gain: 1e-3,
            wmax: 1e-3,
            bass_avg: 0.0,
            last_beat: None,
        })
    }

    pub fn playing(&self) -> bool {
        self.shared.lock().unwrap().last_sound.is_some_and(|t| t.elapsed().as_secs_f32() < 2.0)
    }

    pub fn analyze(&mut self) -> Frame {
        let x = self.shared.lock().unwrap().buf.clone();
        let mut f = Frame { playing: self.playing(), ..Default::default() };
        let mut input: Vec<f32> = x.iter().zip(&self.window).map(|(a, w)| a * w).collect();
        let mut spec = self.fft.make_output_vec();
        let _ = self.fft.process(&mut input, &mut spec);
        let mut cs = vec![0.0f32; spec.len() + 1];
        for (i, c) in spec.iter().enumerate() {
            cs[i + 1] = cs[i] + c.norm();
        }
        let mut db = [0.0f32; BANDS];
        for b in 0..BANDS {
            let (a, e) = (self.edges[b].min(spec.len()), self.edges[b + 1].min(spec.len()));
            let mean = if e > a { (cs[e] - cs[a]) / (e - a) as f32 } else { 0.0 };
            db[b] = 20.0 * (mean + 1e-9).log10();
        }
        let maxdb = db.iter().cloned().fold(f32::MIN, f32::max);
        self.gain = (self.gain * 0.995).max(maxdb); // slow auto-gain
        let v: [f32; BANDS] = std::array::from_fn(|b| {
            if f.playing { ((db[b] - (self.gain - 50.0)) / 50.0).clamp(0.0, 1.0) } else { 0.0 }
        });
        for b in 0..BANDS {
            self.level[b] = v[b].max(self.level[b] * 0.8);
        }
        f.bands = self.level;

        let bars = f.bars();
        for i in 0..32 {
            if bars[i] >= self.peak[i] {
                self.peak[i] = bars[i];
                self.peak_t[i] = Some(Instant::now());
            } else if self.peak_t[i].is_none_or(|t| t.elapsed().as_secs_f32() > 0.6) {
                self.peak[i] = bars[i].max(self.peak[i] - 0.03);
            }
        }
        f.peaks = self.peak;

        let w: Vec<f32> = x[N - 256..].iter().step_by(2).cloned().collect();
        let m = w.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        self.wmax = (self.wmax * 0.98).max(m).max(1e-3);
        if f.playing {
            for (i, v) in w.iter().enumerate().take(128) {
                f.wave[i] = v / self.wmax;
            }
        }

        f.loud = self.level.iter().sum::<f32>() / BANDS as f32;
        f.bass = v[..10].iter().sum::<f32>() / 10.0;
        f.beat = f.playing
            && f.bass > self.bass_avg * 1.25 + 0.06
            && self.last_beat.is_none_or(|t| t.elapsed().as_secs_f32() > 0.3);
        if f.beat {
            self.last_beat = Some(Instant::now());
        }
        self.bass_avg = self.bass_avg * 0.95 + f.bass * 0.05;
        f
    }
}
