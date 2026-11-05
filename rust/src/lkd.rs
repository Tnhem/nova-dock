//! Pioneer car-stereo .lkd animations (format decoded by youxufkhan/carozerra, MIT).
//!
//! Layout: "zLKD" magic, u32 version/unknown/unknown/frame count (20 bytes), then a gzip'd tar holding
//! one 24-bit BMP with all frames stacked top to bottom.

use crate::canvas::{bayer, Canvas};
use crate::config::Config;
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// One RGB frame.
pub struct Rgb {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[u8; 3]>,
}

pub fn frame_count(path: &Path) -> u32 {
    let mut head = [0u8; 20];
    match std::fs::File::open(path).and_then(|mut f| f.read_exact(&mut head)) {
        Ok(()) if &head[..4] == b"zLKD" => u32::from_le_bytes(head[16..20].try_into().unwrap()),
        _ => 0,
    }
}

pub fn read_frames(path: &Path) -> anyhow::Result<Vec<Rgb>> {
    let data = std::fs::read(path)?;
    anyhow::ensure!(data.len() > 20 && &data[..4] == b"zLKD", "not an .lkd file");
    let count = u32::from_le_bytes(data[16..20].try_into()?).max(1) as usize;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(&data[20..]));
    let mut bmp = vec![];
    for entry in tar.entries()? {
        let mut e = entry?;
        if e.header().entry_type().is_file() {
            e.read_to_end(&mut bmp)?;
            break;
        }
    }
    // 24-bit BMP, bottom-up when height > 0
    anyhow::ensure!(bmp.len() > 54 && &bmp[..2] == b"BM", "no bitmap inside");
    let off = u32::from_le_bytes(bmp[10..14].try_into()?) as usize;
    let w = i32::from_le_bytes(bmp[18..22].try_into()?) as usize;
    let hs = i32::from_le_bytes(bmp[22..26].try_into()?);
    anyhow::ensure!(u16::from_le_bytes(bmp[28..30].try_into()?) == 24, "unsupported bitmap depth");
    let h = hs.unsigned_abs() as usize;
    let stride = (w * 3 + 3) & !3;
    let row = |y: usize| {
        let src_y = if hs > 0 { h - 1 - y } else { y };
        off + src_y * stride
    };
    let fh = h / count;
    let mut frames = Vec::with_capacity(count);
    for i in 0..count {
        let mut px = Vec::with_capacity(w * fh);
        for y in i * fh..(i + 1) * fh {
            let r = row(y);
            for x in 0..w {
                let p = r + x * 3;
                px.push(bmp.get(p..p + 3).map_or([0; 3], |b| [b[2], b[1], b[0]]));
            }
        }
        frames.push(Rgb { w, h: fh, px });
    }
    Ok(frames)
}

fn lum(p: [u8; 3]) -> f32 {
    (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) / 255.0
}

/// Pioneer art uses a handful of colours (black + 3 shades of cyan, or 4 greys). Map each colour to an
/// evenly spaced grey level by brightness rank so the tones stay distinct after dithering.
/// None for photo-like clips (use plain luminance).
pub fn tone_map(frames: &[Rgb]) -> Option<HashMap<[u8; 3], f32>> {
    let mut colours = std::collections::HashSet::new();
    for f in frames.iter().step_by((frames.len() / 10).max(1)) {
        for p in &f.px {
            colours.insert(*p);
            if colours.len() > 8 {
                return None;
            }
        }
    }
    let mut c: Vec<[u8; 3]> = colours.into_iter().collect();
    c.sort_by(|a, b| lum(*a).partial_cmp(&lum(*b)).unwrap());
    let n = (c.len().max(2) - 1) as f32;
    Some(c.iter().enumerate().map(|(i, p)| (*p, i as f32 / n)).collect())
}

/// Resample a grey image: box filter when shrinking, bilinear when enlarging (like PIL).
fn resample(src: &[f32], sw: usize, sh: usize, tw: usize, th: usize) -> Vec<f32> {
    let axis = |s: usize, t: usize| -> Vec<Vec<(usize, f32)>> {
        let scale = s as f32 / t as f32;
        (0..t)
            .map(|o| {
                if scale >= 1.0 {
                    let (a, b) = (o as f32 * scale, (o + 1) as f32 * scale);
                    let mut w = vec![];
                    let mut i = a.floor() as usize;
                    while (i as f32) < b && i < s {
                        let cov = (b.min(i as f32 + 1.0) - a.max(i as f32)).max(0.0);
                        if cov > 0.0 {
                            w.push((i, cov / scale));
                        }
                        i += 1;
                    }
                    w
                } else {
                    let p = ((o as f32 + 0.5) * scale - 0.5).clamp(0.0, (s - 1) as f32);
                    let i = p.floor() as usize;
                    let fr = p - i as f32;
                    if i + 1 < s { vec![(i, 1.0 - fr), (i + 1, fr)] } else { vec![(i, 1.0)] }
                }
            })
            .collect()
    };
    let (wx, wy) = (axis(sw, tw), axis(sh, th));
    let mut tmp = vec![0.0; tw * sh];
    for y in 0..sh {
        for (x, ws) in wx.iter().enumerate() {
            tmp[y * tw + x] = ws.iter().map(|&(i, w)| src[y * sw + i] * w).sum();
        }
    }
    let mut out = vec![0.0; tw * th];
    for (y, ws) in wy.iter().enumerate() {
        for x in 0..tw {
            out[y * tw + x] = ws.iter().map(|&(i, w)| tmp[i * tw + x] * w).sum();
        }
    }
    out
}

/// RGB frame -> w x h canvas, ordered-dithered. fit: whole picture with bars; fill: cover, centre
/// cropped; stretch: squeeze.
pub fn to_oled(frame: &Rgb, mode: &str, w: i32, h: i32, tones: Option<&HashMap<[u8; 3], f32>>) -> Canvas {
    let (sw, sh) = (frame.w, frame.h);
    let (tw, th) = if mode == "stretch" {
        (w as usize, h as usize)
    } else {
        let (a, b) = (w as f32 / sw as f32, h as f32 / sh as f32);
        let s = if mode == "fit" { a.min(b) } else { a.max(b) };
        (((sw as f32 * s).round() as usize).max(1), ((sh as f32 * s).round() as usize).max(1))
    };
    let mut l: Vec<f32> = match tones {
        Some(t) => frame.px.iter().map(|p| *t.get(p).unwrap_or(&lum(*p))).collect(),
        None => frame.px.iter().map(|p| lum(*p)).collect(),
    };
    if (tw, th) != (sw, sh) {
        l = resample(&l, sw, sh, tw, th);
    }
    let mut out = Canvas::new(w, h);
    let (x0, y0) = ((w - tw as i32) / 2, (h - th as i32) / 2);
    for y in 0..th as i32 {
        for x in 0..tw as i32 {
            if l[y as usize * tw + x as usize] > bayer(x, y) {
                out.on(x0 + x, y0 + y);
            }
        }
    }
    out
}

/// Menu group for a clip file name.
pub fn group(name: &str) -> &'static str {
    if name.contains("meter") {
        "Meters"
    } else if name.starts_with("movie") {
        "Movies"
    } else if name.starts_with("color_still") || name.starts_with("still") || name.starts_with("alt_still") {
        "Stills"
    } else if name.starts_with("color") {
        "Colour"
    } else {
        "Alt"
    }
}

/// Plays .lkd clips from a folder into any region of the OLED.
pub struct ClipPlayer {
    folder: PathBuf,
    pub names: Vec<String>,
    animated: Vec<String>,
    cache: Vec<((String, i32, i32, String), Vec<Canvas>)>,
    pos: f32,
    prev_t: Option<f64>,
    pub current: Option<String>,
    auto_t: f64,
    track: Option<String>,
}

impl ClipPlayer {
    pub fn new(folder: PathBuf) -> Self {
        let mut names: Vec<String> = std::fs::read_dir(&folder)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().map(str::to_string))
                    .filter(|n| n.to_lowercase().ends_with(".lkd"))
                    .map(|n| n[..n.len() - 4].to_string())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        let animated = names.iter().filter(|n| frame_count(&folder.join(format!("{n}.lkd"))) > 1).cloned().collect();
        ClipPlayer { folder, names, animated, cache: vec![], pos: 0.0, prev_t: None, current: None, auto_t: 0.0, track: None }
    }

    fn frames(&mut self, name: &str, w: i32, h: i32, fit: &str) -> Option<&Vec<Canvas>> {
        let key = (name.to_string(), w, h, fit.to_string());
        if let Some(i) = self.cache.iter().position(|(k, _)| *k == key) {
            let e = self.cache.remove(i);
            self.cache.push(e);
        } else {
            let src = read_frames(&self.folder.join(format!("{name}.lkd"))).ok()?;
            let tones = tone_map(&src);
            let frames = src.iter().map(|f| to_oled(f, fit, w, h, tones.as_ref())).collect();
            self.cache.push((key, frames));
            if self.cache.len() > 3 {
                self.cache.remove(0);
            }
        }
        self.cache.last().map(|(_, f)| f)
    }

    fn pick(&mut self, cfg: &Config, t: f64, title: &str) -> Option<String> {
        if cfg.clip != "auto" {
            return self.names.contains(&cfg.clip).then(|| cfg.clip.clone());
        }
        let pool = if self.animated.is_empty() { &self.names } else { &self.animated };
        if pool.is_empty() {
            return None;
        }
        let stale = self.current.as_ref().is_none_or(|c| !pool.contains(c));
        if stale || self.track.as_deref() != Some(title) || t - self.auto_t > cfg.clip_auto_seconds as f64 {
            let choices: Vec<&String> = pool.iter().filter(|n| Some(*n) != self.current.as_ref()).collect();
            let choice = if choices.is_empty() { &pool[0] } else { choices[fastrand::usize(..choices.len())] };
            self.current = Some(choice.clone());
            self.track = Some(title.to_string());
            self.auto_t = t;
        }
        self.current.clone()
    }

    /// Draw the current clip into rows top..=bot. False when there is nothing to play.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, c: &mut Canvas, top: i32, bot: i32, t: f64, cfg: &Config, title: &str, loud: f32,
                playing: bool) -> bool {
        let Some(name) = self.pick(cfg, t, title) else { return false };
        if self.current.as_ref() != Some(&name) {
            self.current = Some(name.clone());
            self.pos = 0.0;
        }
        let dt = self.prev_t.map_or(0.0, |p| (t - p).clamp(0.0, 0.2)) as f32;
        self.prev_t = Some(t);
        let speed = cfg.clip_fps as f32 * if cfg.clip_react && playing { 0.4 + 1.6 * loud } else { 1.0 };
        let fit = cfg.clip_fit.clone();
        let (w, h) = (c.w, bot - top + 1);
        let pos = self.pos;
        let Some(frames) = self.frames(&name, w, h, &fit) else { return false };
        let n = frames.len() as f32;
        let new_pos = (pos + dt * speed) % n;
        c.paste(&frames[new_pos as usize], 0, top);
        self.pos = new_pos;
        true
    }
}
