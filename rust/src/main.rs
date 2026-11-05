//! nova-dock (Rust): custom screens for the SteelSeries Arctis Nova Pro base station OLED.
//!
//!   nova-dock.exe              tray app
//!   nova-dock.exe --no-tray    console mode, Ctrl+C restores the stock screen
//!   nova-dock.exe --preview F  render every page / style / font to F (PGM) without the dock
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod audio;
mod canvas;
mod config;
mod device;
mod lkd;
mod media;
mod pages;
mod sensors;
mod settings;
mod text;
mod tray;
mod viz;

use std::sync::OnceLock;
use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Threading::CreateMutexW;

static SHARED: OnceLock<app::Shared> = OnceLock::new();

/// False if nova-dock is already running (named mutex, held until exit).
fn single_instance() -> bool {
    unsafe {
        let _ = CreateMutexW(None, false, w!("Local\\nova-dock")); // leaked on purpose: held until exit
        GetLastError() != ERROR_ALREADY_EXISTS
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--settings") {
        if let Err(e) = settings::run() {
            eprintln!("settings: {e}");
        }
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--preview") {
        preview(args.get(i + 1).map(String::as_str).unwrap_or("preview.pgm"));
        return;
    }
    if !single_instance() {
        eprintln!("already running");
        return;
    }
    let shared = app::new_state();
    let _ = SHARED.set(shared.clone());
    if args.iter().any(|a| a == "--no-tray") {
        unsafe extern "system" fn on_ctrl(_: u32) -> windows::core::BOOL {
            if let Some(s) = SHARED.get() {
                s.lock().unwrap().stop = true;
            }
            true.into()
        }
        unsafe {
            let _ = windows::Win32::System::Console::SetConsoleCtrlHandler(Some(on_ctrl), true);
        }
        app::run(shared);
        return;
    }
    let s2 = shared.clone();
    let render = std::thread::spawn(move || app::run(s2));
    if let Err(e) = tray::run(shared.clone()) {
        eprintln!("tray: {e}");
    }
    shared.lock().unwrap().stop = true;
    let _ = render.join();
}

/// Contact sheet of every font x page and every visualizer style, with fake data (for testing).
fn preview(out: &str) {
    use canvas::{Canvas, H, W};
    let dir = config::data_dir();
    let mut cfg = config::Config::default();
    let status = device::Status { battery: Some(6), connected: Some(true), volume: Some(0.6), ..Default::default() };
    let mut system = sensors::System::new();
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let mut viz = viz::Visualizer::new();
    let mut clips = lkd::ClipPlayer::new(dir.join("clips"));
    let big = text::BigFont::bahnschrift();
    let mut fake = viz::Frame { playing: true, loud: 0.5, bass: 0.7, ..Default::default() };
    for i in 0..64 {
        fake.bands[i] = (0.9 - 0.7 * i as f32 / 63.0 + 0.1 * (i as f32).sin()).clamp(0.0, 1.0);
    }
    for x in 0..128 {
        fake.wave[x] = (x as f32 * 0.15).sin() * 0.7;
    }
    let mut tiles: Vec<Canvas> = vec![];
    for (font, ..) in text::FONTS {
        let text = text::Text::new(&dir.join("fonts"), font);
        for (page, title, artist, clock_font) in [
            ("clock", "", "", "classic"),
            ("clock", "", "", "pixel"),
            ("system", "", "", "classic"),
            ("music", "With A Smile", "SYNDRA & Flerian", "classic"),
            ("music", "\u{6674}\u{5929}", "\u{5468}\u{6770}\u{4f26}", "classic"),
        ] {
            cfg.clock_font = clock_font.into();
            let now = media::NowPlaying { title: title.into(), artist: artist.into(), playing: true };
            let mut c = Canvas::new(W, H);
            let mut ctx = pages::Ctx {
                cfg: &cfg, text: &text, big: &big, status: &status, now: &now, system: Some(&mut system), viz: &mut viz,
                clips: &mut clips, frame: &fake, viz_style: "bars", t: 0.0,
            };
            pages::draw_page(page, &mut c, &mut ctx);
            tiles.push(c);
        }
        let mut c = Canvas::new(W, H);
        pages::overlay_box(&mut c, &text, "VOL", Some(0.6));
        tiles.push(c);
    }
    let text = text::Text::new(&dir.join("fonts"), "pixel-operator");
    cfg.clip = "movie8_f".into();
    for style in viz::STYLES {
        let mut v = viz::Visualizer::new();
        let mut c = Canvas::new(W, H);
        for i in 0..40 {
            c = Canvas::new(W, H);
            let mut f = fake.clone();
            f.beat = i % 15 == 0;
            v.draw(&mut c, style, &f, 0, H - 1, 100.0 + i as f64 / 30.0, &cfg, &mut clips, "");
        }
        tiles.push(c);
    }
    let _ = text;
    // PGM sheet, 6 tiles per row, 2x scale
    let (cols, sc, gap) = (6, 2, 6);
    let rows = tiles.len().div_ceil(cols);
    let (sw, sh) = (cols * (W as usize * sc + gap), rows * (H as usize * sc + gap));
    let mut img = vec![70u8; sw * sh];
    for (i, t) in tiles.iter().enumerate() {
        let (ox, oy) = ((i % cols) * (W as usize * sc + gap), (i / cols) * (H as usize * sc + gap));
        for y in 0..H as usize * sc {
            for x in 0..W as usize * sc {
                img[(oy + y) * sw + ox + x] = if t.get((x / sc) as i32, (y / sc) as i32) { 255 } else { 0 };
            }
        }
    }
    let mut data = format!("P5 {sw} {sh} 255\n").into_bytes();
    data.extend(img);
    let _ = std::fs::write(out, data);
    println!("wrote {out}");
}
