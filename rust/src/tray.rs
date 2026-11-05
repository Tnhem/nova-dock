//! Tray icon + right-click menu on the main thread (Win32 message loop, which also receives the hotkeys),
//! and the host side of the settings window (a child process fed with preview frames over stdin).

use crate::app::Shared;
use crate::canvas::{H, W};
use crate::config;
use crate::text::fonts;
use crate::viz::STYLES;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{CheckMenuItem, IsMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, TrayIconBuilder, TrayIconEvent};
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const PAGE_NAMES: &[(&str, &str)] =
    &[("clock", "Clock"), ("system", "System"), ("music", "Music"), ("visualizer", "Visualizer"), ("pioneer", "Pioneer clips")];
pub const CLIP_NOTES: &[(&str, &str)] = &[("movie8_f", "dolphins"), ("movie5", "carrozzeria logo"), ("movie7", "fight movie")];
const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_NAME: &str = "nova-dock-rs";

pub fn clip_label(name: &str) -> String {
    match CLIP_NOTES.iter().find(|c| c.0 == name) {
        Some((_, note)) => format!("{name} ({note})"),
        None => name.to_string(),
    }
}

// ---------------- start with Windows (HKCU Run key) ----------------
pub fn autostart_enabled() -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey(RUN_KEY)
        .and_then(|k| k.get_value::<String, _>(RUN_NAME))
        .is_ok()
}

pub fn set_autostart(on: bool) {
    let Ok(k) = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey_with_flags(RUN_KEY, winreg::enums::KEY_SET_VALUE)
    else {
        return;
    };
    if on {
        let exe = std::env::current_exe().unwrap_or_default();
        let _ = k.set_value(RUN_NAME, &format!("\"{}\"", exe.display()));
    } else {
        let _ = k.delete_value(RUN_NAME);
    }
}

/// 32x32 icon: a little OLED with spectrum bars.
pub fn icon_rgba(connected: bool) -> Vec<u8> {
    let mut px = vec![0u8; 32 * 32 * 4];
    let mut put = |x: i32, y: i32, c: [u8; 4]| {
        if (0..32).contains(&x) && (0..32).contains(&y) {
            let i = ((y * 32 + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&c);
        }
    };
    for y in 5..27 {
        for x in 1..31 {
            let edge = y == 5 || y == 26 || x == 1 || x == 30;
            put(x, y, if edge { [90, 95, 110, 255] } else { [20, 22, 28, 255] });
        }
    }
    let color = if connected { [120, 200, 255, 255] } else { [110, 110, 110, 255] };
    for (i, h) in [7, 13, 10, 16, 9, 12, 6].iter().enumerate() {
        let x = 5 + i as i32 * 3;
        for y in (23 - h)..=23 {
            for dx in 0..2 {
                put(x + dx, y, color);
            }
        }
    }
    px
}

// ---------------- settings window host ----------------
#[derive(Default)]
struct SettingsHost {
    child: Option<Child>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
}

impl SettingsHost {
    fn open(&mut self, shared: &Shared) {
        if let Some(c) = &mut self.child {
            if c.try_wait().ok().flatten().is_none() {
                if let Some(s) = self.stdin.lock().unwrap().as_mut() {
                    let _ = writeln!(s, "RAISE");
                }
                return;
            }
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let Ok(mut child) = Command::new(exe).arg("--settings").stdin(Stdio::piped()).stdout(Stdio::piped()).spawn() else {
            return;
        };
        *self.stdin.lock().unwrap() = child.stdin.take();
        // child -> us: page / pause commands
        if let Some(out) = child.stdout.take() {
            let sh = shared.clone();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                    let mut s = sh.lock().unwrap();
                    match line.split_once(' ') {
                        Some(("PAGE", n)) => s.step_page(n.parse().unwrap_or(1)),
                        Some(("PAUSE", v)) => s.set_paused(v == "1"),
                        _ => {}
                    }
                }
            });
        }
        // us -> child: preview frame + status, 10 times a second while it is open
        let (sh, stdin) = (shared.clone(), self.stdin.clone());
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let msg = {
                let s = sh.lock().unwrap();
                let mut hex = String::with_capacity(W as usize * H as usize / 4);
                for chunk in s.frame.px.chunks(4) {
                    let n = chunk.iter().enumerate().fold(0u8, |a, (i, b)| a | ((*b as u8) << (3 - i)));
                    hex.push(char::from_digit(n as u32, 16).unwrap());
                }
                let status = if !s.connected {
                    "Base station not found, retrying…".to_string()
                } else if s.paused {
                    "Showing the SteelSeries screen".to_string()
                } else if s.page == "pioneer" || (crate::pages::ANIMATED.contains(&s.page.as_str()) && s.viz_style == "pioneer") {
                    format!("Page: {} · {}", s.page, s.clip.clone().unwrap_or_default())
                } else if crate::pages::ANIMATED.contains(&s.page.as_str()) {
                    format!("Page: {} · {}", s.page, s.viz_style)
                } else {
                    format!("Page: {}", s.page)
                };
                format!("FRAME {hex}\nSTATUS {status}\nPAUSED {}\n", s.paused as u8)
            };
            let mut g = stdin.lock().unwrap();
            let Some(w) = g.as_mut() else { break };
            if w.write_all(msg.as_bytes()).and_then(|_| w.flush()).is_err() {
                *g = None;
                break;
            }
        });
        self.child = Some(child);
    }

    fn close(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
        }
    }
}

// ---------------- tray menu ----------------
enum Action {
    Settings,
    Page(String),
    Set(&'static str, Value),
    Toggle(&'static str),
    Pause,
    Autostart,
    OpenConfig,
    Quit,
}

enum Check {
    Page(String),
    Is(&'static str, Value),
    Flag(&'static str),
    Paused,
    Autostart,
}

struct TrayMenu {
    actions: HashMap<MenuId, Action>,
    checks: Vec<(CheckMenuItem, Check)>,
    pages: Vec<(CheckMenuItem, String)>,
}

impl TrayMenu {
    fn item(&mut self, label: &str, a: Action) -> MenuItem {
        let i = MenuItem::new(label, true, None);
        self.actions.insert(i.id().clone(), a);
        i
    }

    fn check(&mut self, label: &str, a: Action, c: Check) -> CheckMenuItem {
        let i = CheckMenuItem::new(label, true, false, None);
        self.actions.insert(i.id().clone(), a);
        self.checks.push((i.clone(), c));
        i
    }

    fn radio(&mut self, label: &str, key: &'static str, v: Value) -> CheckMenuItem {
        self.check(label, Action::Set(key, v.clone()), Check::Is(key, v))
    }

    fn sync(&self, shared: &Shared) {
        let s = shared.lock().unwrap();
        let cfg = serde_json::to_value(&s.cfg).unwrap_or_default();
        for (item, c) in &self.checks {
            item.set_checked(match c {
                Check::Page(p) => s.page == *p,
                Check::Is(k, v) => cfg[*k] == *v,
                Check::Flag(k) => cfg[*k] == Value::Bool(true),
                Check::Paused => s.paused,
                Check::Autostart => autostart_enabled(),
            });
        }
        for (item, p) in &self.pages {
            item.set_enabled(s.cfg.pages.contains(p));
        }
    }
}

fn build_menu(shared: &Shared) -> anyhow::Result<(Menu, TrayMenu)> {
    let mut m = TrayMenu { actions: HashMap::new(), checks: vec![], pages: vec![] };
    let menu = Menu::new();
    let sep = PredefinedMenuItem::separator;

    let settings = m.item("Settings…", Action::Settings);
    let page = Submenu::new("Page", true);
    for (p, label) in PAGE_NAMES {
        let i = m.check(label, Action::Page(p.to_string()), Check::Page(p.to_string()));
        m.pages.push((i.clone(), p.to_string()));
        page.append(&i)?;
    }
    let vis = Submenu::new("Visualizer", true);
    for s in std::iter::once(&"auto").chain(STYLES.iter()) {
        let mut label = s.to_string();
        label[..1].make_ascii_uppercase();
        vis.append(&m.radio(&label, "visualizer", json!(s)))?;
    }
    let clip = Submenu::new("Pioneer clip", true);
    clip.append(&m.radio("Auto (shuffle)", "clip", json!("auto")))?;
    clip.append(&sep())?;
    let names = shared.lock().unwrap().clip_names.clone();
    let mut groups: Vec<(&str, Submenu)> = vec![];
    for n in &names {
        let g = crate::lkd::group(n);
        if !groups.iter().any(|(k, _)| *k == g) {
            groups.push((g, Submenu::new(g, true)));
        }
        let sub = &groups.iter().find(|(k, _)| *k == g).unwrap().1;
        sub.append(&m.radio(&clip_label(n), "clip", json!(n)))?;
    }
    for (_, sub) in &groups {
        clip.append(sub)?;
    }
    let font = Submenu::new("Font", true);
    for f in fonts() {
        font.append(&m.radio(&f.label, "font", json!(f.key)))?;
    }
    let clock = Submenu::new("Big clock", true);
    clock.append(&m.radio("Classic", "clock_font", json!("classic")))?;
    clock.append(&m.radio("Pixel (uses the font)", "clock_font", json!("pixel")))?;
    let bright = Submenu::new("Brightness", true);
    for i in 1..=10 {
        bright.append(&m.radio(&i.to_string(), "brightness", json!(i)))?;
    }
    let auto_music = m.check("Show music while playing", Action::Toggle("auto_music"), Check::Flag("auto_music"));
    let secs = m.check("Clock seconds bar", Action::Toggle("clock_seconds_bar"), Check::Flag("clock_seconds_bar"));
    let pause = m.check("Show SteelSeries screen", Action::Pause, Check::Paused);
    let auto = m.check("Start with Windows", Action::Autostart, Check::Autostart);
    let open = m.item("Open config.json", Action::OpenConfig);
    let quit = m.item("Quit", Action::Quit);
    let (s1, s2, s3) = (sep(), sep(), sep());
    let items: Vec<&dyn IsMenuItem> = vec![
        &settings, &s1, &page, &vis, &clip, &font, &clock, &bright, &auto_music, &secs, &s2, &pause, &auto, &open, &s3,
        &quit,
    ];
    menu.append_items(&items)?;
    Ok((menu, m))
}

/// Main-thread entry: tray icon + hotkeys. Returns when the user quits.
pub fn run(shared: Shared) -> anyhow::Result<()> {
    let (menu, tm) = build_menu(&shared)?;
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip("nova-dock: connecting…")
        .with_icon(Icon::from_rgba(icon_rgba(false), 32, 32)?)
        .build()?;
    let mut host = SettingsHost::default();
    unsafe {
        let mods = MOD_CONTROL | MOD_ALT | MOD_NOREPEAT;
        for (id, vk) in [(1, VK_RIGHT), (2, VK_LEFT), (3, VK_DOWN), (4, VK_UP)] {
            let _ = RegisterHotKey(None, id, mods, vk.0 as u32);
        }
        SetTimer(None, 0, 250, None); // wake up regularly to sync the menu
    }
    let mut connected = None;
    tm.sync(&shared);
    let mut msg = MSG::default();
    'outer: while unsafe { GetMessageW(&mut msg, None, 0, 0) }.0 > 0 {
        if msg.message == WM_HOTKEY {
            shared.lock().unwrap().on_key(msg.wParam.0);
        }
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
            if let TrayIconEvent::DoubleClick { .. } = ev {
                host.open(&shared);
            }
        }
        while let Ok(MenuEvent { id }) = MenuEvent::receiver().try_recv() {
            match tm.actions.get(&id) {
                Some(Action::Settings) => host.open(&shared),
                Some(Action::Page(p)) => shared.lock().unwrap().show_page(p),
                Some(Action::Set(k, v)) => config::set(k, v.clone()),
                Some(Action::Toggle(k)) => {
                    let cur = serde_json::to_value(&shared.lock().unwrap().cfg).unwrap_or_default()[*k] == Value::Bool(true);
                    config::set(k, !cur);
                }
                Some(Action::Pause) => {
                    let mut s = shared.lock().unwrap();
                    let p = !s.paused;
                    s.set_paused(p);
                }
                Some(Action::Autostart) => set_autostart(!autostart_enabled()),
                Some(Action::OpenConfig) => {
                    let _ = Command::new("cmd").args(["/c", "start", "", &config::config_path().display().to_string()]).spawn();
                }
                Some(Action::Quit) => break 'outer,
                None => {}
            }
            tm.sync(&shared);
        }
        if msg.message == WM_TIMER {
            let (changed, conn, page) = {
                let mut s = shared.lock().unwrap();
                let c = s.changed;
                s.changed = false;
                (c, s.connected, s.page.clone())
            };
            if changed {
                tm.sync(&shared);
                if connected != Some(conn) {
                    connected = Some(conn);
                    let _ = tray.set_icon(Icon::from_rgba(icon_rgba(conn), 32, 32).ok());
                }
                let tip = if conn { format!("nova-dock: showing {page}") } else { "nova-dock: base station not found".into() };
                let _ = tray.set_tooltip(Some(tip));
            }
            if shared.lock().unwrap().stop {
                break;
            }
        }
    }
    host.close();
    Ok(())
}
