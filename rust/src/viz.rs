//! Visualizer styles. Each style draws into rows top..=bot of the canvas.

use crate::canvas::{bayer, Canvas, W};
use crate::config::Config;
use crate::lkd::ClipPlayer;
use std::f32::consts::PI;

pub const STYLES: &[&str] =
    &["bars", "segments", "mirror", "mountain", "scope", "waterfall", "radial", "warp", "dolphin", "pioneer"];

/// One audio analysis result. Everything is 0..1 except wave (-1..1).
#[derive(Clone)]
pub struct Frame {
    pub bands: [f32; 64],
    pub peaks: [f32; 32],
    pub wave: [f32; 128],
    pub loud: f32,
    pub bass: f32,
    pub beat: bool,
    pub playing: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Frame { bands: [0.0; 64], peaks: [0.0; 32], wave: [0.0; 128], loud: 0.0, bass: 0.0, beat: false, playing: false }
    }
}

impl Frame {
    pub fn bars(&self) -> [f32; 32] {
        std::array::from_fn(|i| self.bands[2 * i].max(self.bands[2 * i + 1]))
    }

    /// 64 bands interpolated to one value per column.
    pub fn cols(&self) -> [f32; 128] {
        std::array::from_fn(|x| {
            let p = x as f32 * 63.0 / 127.0;
            let i = (p as usize).min(62);
            let fr = p - i as f32;
            self.bands[i] * (1.0 - fr) + self.bands[i + 1] * fr
        })
    }
}

// ---------------- dolphin (Pioneer car stereo style) ----------------
// silhouette facing right, centred on (0, 0), ~36 px long; y grows downward
const DOLPHIN: &[(f32, f32)] = &[
    (18.0, 1.0), (15.0, 0.0), (13.0, -2.5), (10.0, -4.0), (3.0, -5.0), (-3.0, -10.5), (-3.5, -4.6), (-8.0, -3.6),
    (-13.0, -1.6), (-16.0, -5.5), (-18.5, -6.0), (-16.5, -1.0), (-15.8, 0.0), (-16.5, 1.0), (-18.5, 5.5),
    (-16.0, 5.0), (-13.0, 1.6), (-6.0, 3.6), (-1.0, 4.2), (-3.5, 8.0), (1.5, 4.6), (6.0, 4.2), (12.0, 2.2), (15.5, 1.8),
];
const SS: i32 = 4; // supersampling for clean edges at any rotation

/// 1-bit sprite of the dolphin rotated by `angle` degrees (nose up positive).
fn dolphin_sprite(angle: f32, facing: f32, scale: f32) -> Canvas {
    let a = angle.to_radians();
    let (ca, sa) = (a.cos(), a.sin());
    let size = ((44.0 * scale) as i32) | 1;
    let c = (size * SS) as f32 / 2.0;
    let rot = |x: f32, y: f32| {
        let (x, y) = (x * facing * scale, y * scale);
        (c + (x * ca + y * sa * facing) * SS as f32, c + (-x * sa * facing + y * ca) * SS as f32)
    };
    let pts: Vec<(f32, f32)> = DOLPHIN.iter().map(|&(x, y)| rot(x, y)).collect();
    let mut big = Canvas::new(size * SS, size * SS);
    big.polygon(&pts, true);
    let (ex, ey) = rot(10.5, -1.2); // eye
    big.ellipse(ex - 3.0, ey - 3.0, ex + 3.0, ey + 3.0, Some(false));
    // box-downsample, keep cells at least ~43% covered (PIL version: >= 110/255)
    let mut out = Canvas::new(size, size);
    for y in 0..size {
        for x in 0..size {
            let mut n = 0;
            for dy in 0..SS {
                for dx in 0..SS {
                    n += big.get(x * SS + dx, y * SS + dy) as i32;
                }
            }
            if n * 255 >= 110 * SS * SS {
                out.on(x, y);
            }
        }
    }
    out
}

struct Jump {
    x0: f32,
    len: f32,
    height: f32,
    t0: f64,
    dur: f32,
}

/// A dolphin swims along the sea, fin above water, and leaps on beats.
/// The sea surface swells with loudness, and splashes leave ripples.
pub struct Dolphin {
    x: f32,
    dir: f32,
    jump: Option<Jump>,
    last_end: f64,
    prev_t: Option<f64>,
    parts: Vec<[f32; 4]>,
    ripples: Vec<(f32, f64)>,
    splashed: [bool; 2],
    scale: f32,
    stars: Vec<(i32, f32)>,
}

impl Dolphin {
    pub fn new() -> Self {
        Dolphin {
            x: 30.0,
            dir: 1.0,
            jump: None,
            last_end: -10.0,
            prev_t: None,
            parts: vec![],
            ripples: vec![],
            splashed: [false; 2],
            scale: 1.0,
            stars: (0..9).map(|_| (fastrand::i32(0..W), fastrand::f32())).collect(),
        }
    }

    fn surface(&mut self, base: f32, loud: f32, t: f64) -> [f32; 128] {
        let amp = 0.6 + 2.5 * loud;
        let tf = t as f32;
        let mut s: [f32; 128] = std::array::from_fn(|x| {
            let x = x as f32;
            base + amp * (x * 0.17 - (t * 3.5 % 6283.0) as f32).sin() + 0.7 * amp * (x * 0.061 + (t * 1.6 % 6283.0) as f32).sin()
        });
        let _ = tf;
        for &(rx, t0) in &self.ripples {
            let age = (t - t0) as f32;
            for (x, v) in s.iter_mut().enumerate() {
                let d = (x as f32 - rx).abs();
                if d < age * 45.0 {
                    *v += 2.2 * (d * 0.55 - age * 11.0).cos() * (-age * 1.8).exp();
                }
            }
        }
        self.ripples.retain(|r| t - r.1 < 2.5);
        s
    }

    fn splash(&mut self, x: f32, y: f32, t: f64, power: f32) {
        for _ in 0..(8.0 + 8.0 * power) as i32 {
            self.parts.push([
                x + fastrand::f32() * 6.0 - 3.0,
                y,
                fastrand::f32() * 56.0 - 28.0,
                -(25.0 + fastrand::f32() * 30.0) * (0.6 + power * 0.6),
            ]);
        }
        self.ripples.push((x, t));
    }

    fn start_jump(&mut self, base: f32, top: f32, t: f64, power: f32) {
        let height = (base + 5.0 - (top + 12.0 * self.scale)).min(10.0 + (base - top) * 0.6 * power);
        let mut len = 30.0 + 35.0 * power;
        let end = self.x + self.dir * len;
        if !(6.0 < end && end < W as f32 - 6.0) {
            self.dir = -self.dir;
            len = len.min(if self.dir > 0.0 { W as f32 - 8.0 - self.x } else { self.x - 8.0 });
        }
        if len < 20.0 {
            return;
        }
        self.jump = Some(Jump { x0: self.x, len, height, t0: t, dur: 0.85 + 0.45 * power });
        self.splashed = [false; 2];
    }

    pub fn draw(&mut self, c: &mut Canvas, f: &Frame, top: i32, bot: i32, t: f64) {
        let dt = self.prev_t.map_or(0.0, |p| (t - p).clamp(0.0, 0.1)) as f32;
        self.prev_t = Some(t);
        let base = (bot - ((bot - top) / 4).max(10)) as f32;
        self.scale = ((bot - top) as f32 / 55.0).min(1.0);
        let surf = self.surface(base, f.loud, t);
        let tf = (t % 10000.0) as f32;

        // sky: moon + twinkling stars
        c.ellipse((W - 18) as f32, (top + 2) as f32, (W - 9) as f32, (top + 11) as f32, Some(true));
        c.ellipse((W - 21) as f32, (top + 1) as f32, (W - 13) as f32, (top + 9) as f32, Some(false));
        for (i, &(sx, sy)) in self.stars.iter().enumerate() {
            let y = top + 1 + (sy * (base - top as f32 - 14.0)) as i32;
            if (tf * 2.0 + i as f32 * 1.7).sin() > -0.3 {
                c.on(sx, y);
            }
        }

        // sea surface and drifting wave lines below it
        let line: Vec<(i32, i32)> = (0..W).map(|x| (x, surf[x as usize].round() as i32)).collect();
        c.polyline(&line);
        let mut k = 0;
        let mut y = base as i32 + 5;
        while y <= bot {
            let off = tf * (8.0 + 4.0 * k as f32) * if k % 2 == 1 { 1.0 } else { -1.0 };
            let (period, dash) = (14.0 + 4.0 * k as f32, 6.0 + 2.0 * k as f32);
            for x in 0..W {
                if (x as f32 + off).rem_euclid(period) < dash {
                    c.on(x, y + (x as f32 * 0.2 + tf * 2.0 + k as f32).sin().round() as i32);
                }
            }
            k += 1;
            y += 4;
        }

        // behaviour
        if self.jump.is_none() {
            self.x += self.dir * 16.0 * dt;
            if self.x > (W - 10) as f32 || self.x < 10.0 {
                self.dir = -self.dir;
                self.x = self.x.clamp(10.0, (W - 10) as f32);
            }
            let idle = if f.playing { 9.0 } else { 14.0 };
            if (f.beat && t - self.last_end > 0.7) || t - self.last_end > idle {
                let power = if f.beat { (0.35 + f.bass).min(1.0) } else { 0.3 };
                self.start_jump(base, top as f32, t, power);
            }
        }
        let surf_at = |x: f32| surf[(x.round() as i32).clamp(0, W - 1) as usize];
        match &self.jump {
            None => {
                // swimming: only the dorsal fin cuts the surface
                let (x, d) = (self.x.round(), self.dir);
                let s = surf_at(x).round();
                c.polygon(&[(x - 4.0 * d, s), (x + 3.0 * d, s), (x - 2.0 * d, s - 5.0), (x - 3.0 * d, s - 4.0)], true);
            }
            Some(j) => {
                let p = ((t - j.t0) as f32) / j.dur;
                if p >= 1.0 {
                    self.x = j.x0 + self.dir * j.len;
                    self.jump = None;
                    self.last_end = t;
                } else {
                    let (x0, len, height) = (j.x0, j.len, j.height);
                    let x = x0 + self.dir * len * p;
                    let y = base + 5.0 - 4.0 * height * p * (1.0 - p);
                    let ang = (4.0 * height * (1.0 - 2.0 * p)).atan2(len).to_degrees();
                    for (i, mark) in [0.12f32, 0.88].iter().enumerate() {
                        if p > *mark && !self.splashed[i] {
                            self.splashed[i] = true;
                            let sy = surf_at(x);
                            self.splash(x, sy, t, height / 30.0);
                        }
                    }
                    let spr = dolphin_sprite(ang, self.dir, self.scale);
                    let (ox, oy) = ((x - spr.w as f32 / 2.0).round() as i32, (y - spr.h as f32 / 2.0).round() as i32);
                    for sy in 0..spr.h {
                        for sx in 0..spr.w {
                            let (px, py) = (ox + sx, oy + sy);
                            // the part still under water is hidden
                            if spr.get(sx, sy) && px >= 0 && px < W && (py as f32) < surf[px as usize] {
                                c.on(px, py);
                            }
                        }
                    }
                }
            }
        }

        // droplets
        let mut alive = Vec::with_capacity(self.parts.len());
        for mut pt in self.parts.drain(..) {
            pt[3] += 90.0 * dt;
            pt[0] += pt[2] * dt;
            pt[1] += pt[3] * dt;
            let xi = pt[0].round() as i32;
            if (0..W).contains(&xi) && pt[1] < surf[xi as usize] + 1.0 {
                c.on(xi, pt[1].round() as i32);
                alive.push(pt);
            }
        }
        self.parts = alive;
    }
}

// ---------------- styles ----------------
pub struct Visualizer {
    fall: Vec<[f32; 128]>,
    stars: Vec<[f32; 3]>,
    prev_t: Option<f64>,
    dolphin: Dolphin,
}

impl Visualizer {
    pub fn new() -> Self {
        Visualizer { fall: vec![], stars: vec![], prev_t: None, dolphin: Dolphin::new() }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, c: &mut Canvas, style: &str, f: &Frame, top: i32, bot: i32, t: f64, cfg: &Config,
                clips: &mut ClipPlayer, title: &str) {
        let peaks = cfg.visualizer_peaks;
        match style {
            "dolphin" => self.dolphin.draw(c, f, top, bot, t),
            "pioneer" => {
                if !clips.draw(c, top, bot, t, cfg, title, f.loud, f.playing) {
                    bars(c, f, top, bot, peaks)
                }
            }
            "segments" => segments(c, f, top, bot, peaks),
            "mirror" => mirror(c, f, top, bot),
            "mountain" => mountain(c, f, top, bot),
            "scope" => scope(c, f, top, bot),
            "waterfall" => self.waterfall(c, f, top, bot),
            "radial" => radial(c, f, top, bot),
            "warp" => self.warp(c, f, top, bot, t),
            _ => bars(c, f, top, bot, peaks),
        }
        self.prev_t = Some(t);
    }

    /// Scrolling spectrogram: newest line on top, flowing down.
    fn waterfall(&mut self, c: &mut Canvas, f: &Frame, top: i32, bot: i32) {
        let span = (bot - top + 1) as usize;
        if self.fall.len() != span {
            self.fall = vec![[0.0; 128]; span];
        }
        self.fall.rotate_right(1);
        self.fall[0] = f.cols().map(|v| v.powf(1.3));
        for (r, row) in self.fall.iter().enumerate() {
            let y = top + r as i32;
            for x in 0..W {
                c.set(x, y, row[x as usize] > bayer(x, r as i32));
            }
        }
    }

    /// Starfield that speeds up with the music and lurches forward on beats.
    fn warp(&mut self, c: &mut Canvas, f: &Frame, top: i32, bot: i32, t: f64) {
        let dt = self.prev_t.map_or(0.0, |p| (t - p).clamp(0.0, 0.1)) as f32;
        let rnd = || fastrand::f32() * 2.0 - 1.0;
        if self.stars.is_empty() {
            self.stars = (0..90).map(|_| [rnd(), rnd(), 0.05 + fastrand::f32() * 0.95]).collect();
        }
        let (cx, cy) = (W as f32 / 2.0, (top + bot) as f32 / 2.0);
        let hh = (bot - top) as f32 / 2.0;
        let speed = 0.08 + 1.4 * f.loud + if f.beat { 1.2 } else { 0.0 };
        for s in self.stars.iter_mut() {
            let mut old = *s;
            s[2] -= speed * dt;
            if s[2] <= 0.04 || (s[0] / s[2]).abs() > 1.2 || (s[1] / s[2]).abs() > 1.2 {
                *s = [rnd(), rnd(), 1.0];
                old = *s;
            }
            let p = (cx + s[0] / s[2] * 64.0, cy + s[1] / s[2] * hh);
            let q = (cx + old[0] / old[2] * 64.0, cy + old[1] / old[2] * hh);
            if (top as f32) <= p.1 && p.1 <= bot as f32 {
                c.linef(q.0, q.1, p.0, p.1);
            }
        }
    }
}

fn bars(c: &mut Canvas, f: &Frame, top: i32, bot: i32, peaks: bool) {
    let span = (bot - top + 1) as f32;
    for (i, (v, pk)) in f.bars().iter().zip(f.peaks.iter()).enumerate() {
        let (x, h) = (i as i32 * 4, (v * span).round() as i32);
        if h > 0 {
            c.fill_rect(x, bot - h + 1, x + 2, bot, true);
        }
        let ph = (pk * span).round() as i32;
        if peaks && ph > h + 1 {
            c.line(x, bot - ph + 1, x + 2, bot - ph + 1);
        }
    }
}

/// Classic hi-fi LED meter: stacked 2 px blocks.
fn segments(c: &mut Canvas, f: &Frame, top: i32, bot: i32, peaks: bool) {
    let span = (bot - top + 1) as f32;
    for (i, (v, pk)) in f.bars().iter().zip(f.peaks.iter()).enumerate() {
        let x = i as i32 * 4;
        let n = (v * span / 3.0).round() as i32;
        for k in 0..n {
            let y = bot - k * 3;
            c.fill_rect(x, y - 1, x + 2, y, true);
        }
        let pn = (pk * span / 3.0).round() as i32;
        if peaks && pn > n {
            let y = bot - pn * 3;
            c.fill_rect(x, y - 1, x + 2, y, true);
        }
    }
}

/// Bass in the middle, highs at both edges, bars grow up and down from the centre line.
fn mirror(c: &mut Canvas, f: &Frame, top: i32, bot: i32) {
    let half: [f32; 16] = std::array::from_fn(|i| f.bands[i * 4..i * 4 + 4].iter().cloned().fold(0.0, f32::max));
    let mid = (top + bot) as f32 / 2.0;
    for i in 0..32 {
        let v = if i < 16 { half[15 - i] } else { half[i - 16] };
        let h = v * (bot - top) as f32 / 2.0;
        let x = i as i32 * 4;
        c.fill_rect(x, (mid - h).round() as i32, x + 2, (mid + h).round() as i32, true);
    }
}

/// Smooth spectrum outline with a dithered fill.
fn mountain(c: &mut Canvas, f: &Frame, top: i32, bot: i32) {
    let span = (bot - top + 1) as f32;
    let cols = f.cols();
    // 5-tap moving average, zero padded like np.convolve(mode="same")
    let sm: [f32; 128] = std::array::from_fn(|x| {
        (x as i32 - 2..=x as i32 + 2).filter(|i| (0..128).contains(i)).map(|i| cols[i as usize]).sum::<f32>() / 5.0
    });
    let ys: [f32; 128] = std::array::from_fn(|x| bot as f32 - sm[x] * (span - 1.0));
    for x in 0..W {
        let yl = ys[x as usize];
        for y in top..=bot {
            let yf = y as f32;
            if yf > yl {
                let shade = ((yf - yl) / span * 0.9 + 0.15).clamp(0.0, 0.6);
                c.set(x, y, shade > bayer(x, y - top));
            }
        }
    }
    let line: Vec<(i32, i32)> = (0..W).map(|x| (x, ys[x as usize].round() as i32)).collect();
    c.polyline(&line);
}

/// Oscilloscope: the raw waveform.
fn scope(c: &mut Canvas, f: &Frame, top: i32, bot: i32) {
    let (mid, half) = ((top + bot) as f32 / 2.0, (bot - top) as f32 / 2.0);
    let line: Vec<(i32, i32)> = f.wave.iter().enumerate().map(|(x, v)| (x as i32, (mid - v * half).round() as i32)).collect();
    c.polyline(&line);
}

/// Spokes around a ring that pulses with the bass.
fn radial(c: &mut Canvas, f: &Frame, top: i32, bot: i32) {
    let (cx, cy) = (W as f32 / 2.0, (top + bot) as f32 / 2.0);
    let span = (bot - top) as f32;
    let r0 = span * 0.18 + f.bass * span * 0.12;
    let n = 48;
    for i in 0..n {
        let a = 2.0 * PI * i as f32 / n as f32 - PI / 2.0;
        let idx = (((i as f32 - n as f32 / 2.0).abs() / (n as f32 / 2.0)) * 63.0) as usize; // bass at the bottom
        let v = f.bands[idx.min(63)];
        let r1 = r0 + 2.0 + v * span * 0.45;
        let (ca, sa) = (a.cos(), a.sin());
        c.linef(cx + ca * (r0 + 2.0) * 1.6, cy + sa * (r0 + 2.0), cx + ca * r1 * 1.6, cy + sa * r1);
    }
    c.ellipse(cx - r0 * 1.6, cy - r0, cx + r0 * 1.6, cy + r0, None);
}
