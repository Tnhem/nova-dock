//! config.json: typed settings with defaults; unknown keys are preserved when saving.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub pages: Vec<String>,
    pub rotate_seconds: u32,
    pub auto_music: bool,
    pub brightness: u8,
    pub clock_24h: bool,
    pub clock_seconds_bar: bool,
    pub clock_font: String,
    pub font: String,
    pub visualizer: String,
    pub visualizer_auto_seconds: u32,
    pub visualizer_peaks: bool,
    pub clip: String,
    pub clip_fit: String,
    pub clip_fps: u32,
    pub clip_auto_seconds: u32,
    pub clip_react: bool,
    pub audio_device: String,
    pub pixel_shift_minutes: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            pages: vec!["clock".into(), "system".into(), "music".into()],
            rotate_seconds: 0,
            auto_music: true,
            brightness: 6,
            clock_24h: true,
            clock_seconds_bar: true,
            clock_font: "classic".into(),
            font: "pixel-operator".into(),
            visualizer: "auto".into(),
            visualizer_auto_seconds: 45,
            visualizer_peaks: false,
            clip: "auto".into(),
            clip_fit: "fill".into(),
            clip_fps: 16,
            clip_auto_seconds: 60,
            clip_react: false,
            audio_device: String::new(),
            pixel_shift_minutes: 3,
        }
    }
}

pub const PAGES: &[&str] = &["clock", "system", "music", "visualizer", "pioneer"];

/// Folder holding config.json, fonts/ and clips/: $NOVA_DOCK_HOME, else the nearest ancestor of the
/// executable that has a fonts/ folder (so rust/target/release/nova-dock.exe finds the project root).
pub fn data_dir() -> PathBuf {
    if let Ok(h) = std::env::var("NOVA_DOCK_HOME") {
        return PathBuf::from(h);
    }
    let exe = std::env::current_exe().unwrap_or_default();
    let mut d = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    loop {
        if d.join("fonts").is_dir() {
            return d;
        }
        if !d.pop() {
            return exe.parent().map(Path::to_path_buf).unwrap_or_default();
        }
    }
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

fn read_map(path: &Path) -> Option<Map<String, Value>> {
    let text = std::fs::read_to_string(path).ok()?;
    match serde_json::from_str(&text).ok()? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

fn write_map(path: &Path, m: &Map<String, Value>) {
    if let Ok(text) = serde_json::to_string_pretty(m) {
        let _ = std::fs::write(path, text);
    }
}

/// Load, creating the file and adding any missing keys so new settings are discoverable.
pub fn load(prev: Option<&Config>) -> Config {
    let path = config_path();
    let defaults = serde_json::to_value(Config::default()).unwrap();
    let Value::Object(defaults) = defaults else { unreachable!() };
    let user = {
        let _g = LOCK.lock().unwrap();
        read_map(&path)
    };
    let Some(mut user) = user else {
        if path.exists() {
            return prev.cloned().unwrap_or_default(); // half-written / invalid: keep what we had
        }
        save(defaults);
        return Config::default();
    };
    let missing: Vec<_> = defaults.keys().filter(|k| !user.contains_key(*k)).cloned().collect();
    if !missing.is_empty() {
        for k in &missing {
            user.insert(k.clone(), defaults[k].clone());
        }
        save(user.clone());
    }
    let mut cfg: Config = serde_json::from_value(Value::Object(user)).unwrap_or_else(|_| prev.cloned().unwrap_or_default());
    cfg.pages.retain(|p| PAGES.contains(&p.as_str()));
    if cfg.pages.is_empty() {
        cfg.pages = vec!["clock".into()];
    }
    if cfg.visualizer != "auto" && !crate::viz::STYLES.contains(&cfg.visualizer.as_str()) {
        cfg.visualizer = "auto".into();
    }
    if !["fill", "fit", "stretch"].contains(&cfg.clip_fit.as_str()) {
        cfg.clip_fit = "fill".into();
    }
    if !crate::text::fonts().iter().any(|f| f.key == cfg.font) {
        cfg.font = "pixel-operator".into();
    }
    cfg.brightness = cfg.brightness.clamp(1, 10);
    cfg
}

/// Merge changes into config.json; the render loop picks them up on its next frame.
pub fn save(changes: Map<String, Value>) {
    let path = config_path();
    let _g = LOCK.lock().unwrap();
    let mut user = read_map(&path).unwrap_or_default();
    for (k, v) in changes {
        user.insert(k, v);
    }
    write_map(&path, &user);
}

pub fn set(key: &str, value: impl Into<Value>) {
    let mut m = Map::new();
    m.insert(key.to_string(), value.into());
    save(m);
}

pub fn mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}
