//! Render loop plus the state the tray menu and settings window read and change.

use crate::audio::Audio;
use crate::canvas::{Canvas, H, W};
use crate::config::{self, Config};
use crate::device::{pixel_shift, Dock, DockStatus, Status};
use crate::lkd::ClipPlayer;
use crate::pages::{self, Ctx, ALWAYS_MOVING, ANIMATED};
use crate::sensors::System;
use crate::text::{BigFont, Text};
use crate::viz::{Frame, Visualizer, STYLES};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Resolves the visualizer style: fixed, or "auto" = new style per track and every N seconds.
pub struct StylePicker {
    pub over: Option<String>, // set by hotkey: a style name or "auto"
    auto_i: usize,
    auto_t: Instant,
    track: Option<String>,
    label_t: Option<Instant>,
}

impl StylePicker {
    fn new() -> Self {
        StylePicker { over: None, auto_i: 0, auto_t: Instant::now(), track: None, label_t: None }
    }

    fn cycle(&mut self, cfg: &Config) {
        let opts: Vec<&str> = std::iter::once("auto").chain(STYLES.iter().copied()).collect();
        let cur = self.over.clone().unwrap_or(cfg.visualizer.clone());
        let i = opts.iter().position(|o| *o == cur).unwrap_or(0);
        self.over = Some(opts[(i + 1) % opts.len()].to_string());
        self.label_t = Some(Instant::now());
    }

    fn style(&mut self, cfg: &Config, title: &str) -> String {
        let mode = self.over.clone().unwrap_or(cfg.visualizer.clone());
        if mode != "auto" {
            return mode;
        }
        if self.track.as_deref() != Some(title) || self.auto_t.elapsed().as_secs() > cfg.visualizer_auto_seconds as u64 {
            if self.track.is_some() {
                self.auto_i += 1;
            }
            self.track = Some(title.to_string());
            self.auto_t = Instant::now();
        }
        STYLES[self.auto_i % STYLES.len()].to_string()
    }

    fn label(&self, cfg: &Config) -> Option<String> {
        self.label_t
            .filter(|t| t.elapsed() < Duration::from_millis(1500))
            .map(|_| self.over.clone().unwrap_or(cfg.visualizer.clone()))
    }
}

pub struct State {
    pub cfg: Config,
    pub page_i: usize,
    page_t: Instant,
    manual_t: Option<Instant>,
    pub paused: bool,
    pub page: String,          // page currently on screen
    pub frame: Canvas,         // last frame, for the settings preview
    pub connected: bool,
    pub viz_style: String,
    pub clip: Option<String>,
    pub clip_names: Vec<String>,
    pub picker: StylePicker,
    pub stop: bool,
    pub changed: bool,         // tray menu / tooltip need a refresh
}

pub type Shared = Arc<Mutex<State>>;

pub fn new_state() -> Shared {
    Arc::new(Mutex::new(State {
        cfg: config::load(None),
        page_i: 0,
        page_t: Instant::now(),
        manual_t: None,
        paused: false,
        page: String::new(),
        frame: Canvas::screen(),
        connected: false,
        viz_style: "bars".into(),
        clip: None,
        clip_names: vec![],
        picker: StylePicker::new(),
        stop: false,
        changed: true,
    }))
}

// --- controls (called from tray / hotkeys / settings window) ---
impl State {
    pub fn step_page(&mut self, step: i32) {
        let n = self.cfg.pages.len() as i32;
        self.page_i = ((self.page_i as i32 + step).rem_euclid(n)) as usize;
        self.page_t = Instant::now();
        self.manual_t = Some(Instant::now());
    }

    pub fn show_page(&mut self, name: &str) {
        if let Some(i) = self.cfg.pages.iter().position(|p| p == name) {
            self.page_i = i;
            self.page_t = Instant::now();
            self.manual_t = Some(Instant::now());
        }
    }

    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        self.changed = true;
    }

    pub fn on_key(&mut self, k: usize) {
        match k {
            3 => self.paused = !self.paused,
            4 => {
                let cfg = self.cfg.clone();
                self.picker.cycle(&cfg)
            }
            1 => self.step_page(1),
            _ => self.step_page(-1),
        }
        self.changed = true;
    }
}

/// Everything the loop owns (not shared with other threads).
struct Engine {
    text: Text,
    font: String,
    big: BigFont,
    system: Option<System>, // created the first time the system page is shown (NVML costs ~28 MB)
    viz: Visualizer,
    clips: ClipPlayer,
    audio: Option<Audio>,
    audio_tried: bool,
    now: Arc<Mutex<crate::media::NowPlaying>>,
    started: Instant,
}

pub fn run(shared: Shared) {
    let dir = config::data_dir();
    let font = shared.lock().unwrap().cfg.font.clone();
    let mut e = Engine {
        text: Text::new(&dir.join("fonts"), &font),
        font,
        big: BigFont::bahnschrift(),
        system: None,
        viz: Visualizer::new(),
        clips: ClipPlayer::new(dir.join("clips")),
        audio: None,
        audio_tried: false,
        now: crate::media::start(),
        started: Instant::now(),
    };
    shared.lock().unwrap().clip_names = e.clips.names.clone();
    let mut cfg_mtime = config::mtime();
    let mut dock: Option<(Dock, DockStatus)> = None;
    let mut stock_shown = false;
    let mut next_t = Instant::now();

    while !shared.lock().unwrap().stop {
        let step = (|| -> anyhow::Result<Duration> {
            if dock.is_none() {
                let d = Dock::connect()?;
                d.brightness(shared.lock().unwrap().cfg.brightness)?;
                let st = DockStatus::start(&d);
                dock = Some((d, st));
                let mut s = shared.lock().unwrap();
                s.connected = true;
                s.changed = true;
                eprintln!("connected to base station");
            }
            let (d, st) = dock.as_mut().unwrap();
            if e.audio.is_none() && !e.audio_tried {
                e.audio_tried = true;
                let dev = shared.lock().unwrap().cfg.audio_device.clone();
                match Audio::new(&dev) {
                    Ok(a) => e.audio = Some(a),
                    Err(err) => eprintln!("audio capture unavailable: {err}"),
                }
            }

            // config hot reload
            let m = config::mtime();
            if m != cfg_mtime {
                cfg_mtime = m;
                let mut s = shared.lock().unwrap();
                let old = s.cfg.clone();
                s.cfg = config::load(Some(&old));
                s.picker.over = None;
                s.page_i %= s.cfg.pages.len();
                s.changed = true;
                if s.cfg.brightness != old.brightness {
                    d.brightness(s.cfg.brightness)?;
                }
                if s.cfg.audio_device != old.audio_device {
                    e.audio = None;
                    e.audio_tried = false;
                }
                if s.cfg.font != e.font {
                    e.font = s.cfg.font.clone();
                    e.text = Text::new(&dir.join("fonts"), &e.font);
                }
            }

            if shared.lock().unwrap().paused {
                if !stock_shown {
                    d.stock_ui()?;
                    stock_shown = true;
                }
                return Ok(Duration::from_millis(200));
            }
            stock_shown = false;

            let interval = render(&shared, &mut e, &st.get());
            let frame = shared.lock().unwrap().frame.clone();
            d.draw(&frame)?;
            Ok(interval)
        })();
        match step {
            Ok(interval) => {
                // sleep until the next frame is due, so drawing time doesn't lower the frame rate
                next_t = (next_t + interval).max(Instant::now());
                std::thread::sleep(next_t.saturating_duration_since(Instant::now()));
            }
            Err(err) => {
                eprintln!("device error: {err}; retrying");
                dock = None;
                e.audio = None;
                e.audio_tried = false;
                {
                    let mut s = shared.lock().unwrap();
                    s.connected = false;
                    s.changed = true;
                }
                std::thread::sleep(Duration::from_secs(3));
                next_t = Instant::now();
            }
        }
    }
    drop(dock); // Dock::drop restores the stock UI
    eprintln!("restored stock UI");
}

fn render(shared: &Shared, e: &mut Engine, status: &Status) -> Duration {
    let now_playing = e.now.lock().unwrap().clone();
    let playing = e.audio.as_ref().is_some_and(|a| a.playing());
    let (cfg, name, style, label) = {
        let mut s = shared.lock().unwrap();
        let n = s.cfg.pages.len();
        if s.cfg.rotate_seconds > 0 && s.page_t.elapsed().as_secs() > s.cfg.rotate_seconds as u64 {
            s.page_i = (s.page_i + 1) % n;
            s.page_t = Instant::now();
        }
        let mut name = s.cfg.pages[s.page_i % n].clone();
        // auto-switch to music while sound plays, unless a page was picked recently
        let recent = s.manual_t.is_some_and(|t| t.elapsed() < Duration::from_secs(30));
        if s.cfg.auto_music && playing && !recent && !ANIMATED.contains(&name.as_str()) {
            name = "music".into();
        }
        if name != s.page {
            s.page = name.clone();
            s.changed = true;
        }
        let cfg = s.cfg.clone();
        let style = if ANIMATED.contains(&name.as_str()) { s.picker.style(&cfg, &now_playing.title) } else { s.viz_style.clone() };
        s.viz_style = style.clone();
        let label = s.picker.label(&cfg);
        (cfg, name, style, label)
    };

    let frame = if ANIMATED.contains(&name.as_str()) {
        e.audio.as_mut().map(|a| a.analyze()).unwrap_or_default()
    } else {
        Frame::default()
    };
    if name == "system" {
        e.system.get_or_insert_with(System::new);
    }
    let mut c = Canvas::new(W, H);
    let mut ctx = Ctx {
        cfg: &cfg,
        text: &e.text,
        big: &e.big,
        status,
        now: &now_playing,
        system: e.system.as_mut(),
        viz: &mut e.viz,
        clips: &mut e.clips,
        frame: &frame,
        viz_style: &style,
        t: e.started.elapsed().as_secs_f64(),
    };
    pages::draw_page(&name, &mut c, &mut ctx);
    if status.volume_overlay() {
        pages::overlay_box(&mut c, &e.text, "VOL", status.volume);
    } else if let Some(l) = &label {
        pages::overlay_box(&mut c, &e.text, &format!("style: {l}"), None);
    }
    let out = pixel_shift(&c, cfg.pixel_shift_minutes);
    {
        let mut s = shared.lock().unwrap();
        s.frame = out;
        s.clip = e.clips.current.clone();
    }
    // some animations keep moving in silence; the rest only need speed while sound plays
    let fast = name == "pioneer" || (ANIMATED.contains(&name.as_str()) && (playing || ALWAYS_MOVING.contains(&style.as_str())));
    if fast || status.volume_overlay() || label.is_some() {
        Duration::from_micros(33_333)
    } else {
        Duration::from_millis(500)
    }
}
