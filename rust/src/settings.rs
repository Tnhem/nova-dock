//! Settings window (`nova-dock --settings`), started by the tray process.
//! Settings go straight to config.json; the live preview and status arrive on stdin;
//! page / pause buttons are sent back on stdout.

use crate::config::{self, Config, PAGES};
use crate::text::fonts;
use crate::tray::{autostart_enabled, clip_label, icon_rgba, set_autostart, PAGE_NAMES};
use crate::viz::STYLES;
use eframe::egui::{self, Color32, ColorImage, TextureHandle, TextureOptions};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Feed {
    frame: Vec<bool>,
    status: String,
    paused: bool,
    raise: bool,
    ctx: Option<egui::Context>,
}

struct App {
    cfg: Config,
    mtime: Option<std::time::SystemTime>,
    feed: Arc<Mutex<Feed>>,
    tex: Option<TextureHandle>,
    clips: Vec<String>,
    devices: Vec<String>,
    autostart: bool,
}

fn send(line: &str) {
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

fn set(key: &str, v: impl Into<Value>) {
    config::set(key, v);
}

fn combo(ui: &mut egui::Ui, id: &str, current: &str, options: &[(String, String)], width: f32) -> Option<String> {
    let mut picked = None;
    let shown = options.iter().find(|o| o.0 == current).map_or(current.to_string(), |o| o.1.clone());
    egui::ComboBox::from_id_salt(id).width(width).selected_text(shown).show_ui(ui, |ui| {
        for (value, label) in options {
            if ui.selectable_label(value == current, label).clicked() {
                picked = Some(value.clone());
            }
        }
    });
    picked
}

fn number(ui: &mut egui::Ui, key: &str, v: u32, lo: u32, hi: u32) {
    let mut x = v;
    if ui.add(egui::DragValue::new(&mut x).range(lo..=hi)).changed() {
        set(key, x);
    }
}

fn check(ui: &mut egui::Ui, key: &str, v: bool, label: &str) {
    let mut x = v;
    if ui.checkbox(&mut x, label).changed() {
        set(key, x);
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // follow changes made from the tray menu
        let m = config::mtime();
        if m != self.mtime {
            self.mtime = m;
            self.cfg = config::load(Some(&self.cfg));
        }
        let (frame, status, paused, raise) = {
            let mut f = self.feed.lock().unwrap();
            f.ctx.get_or_insert_with(|| ctx.clone());
            let r = f.raise;
            f.raise = false;
            (f.frame.clone(), f.status.clone(), f.paused, r)
        };
        if raise {
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        if frame.len() == 128 * 64 {
            let on = Color32::from_rgb(0xa8, 0xdc, 0xff);
            let img = ColorImage {
                size: [128, 64],
                pixels: frame.iter().map(|b| if *b { on } else { Color32::BLACK }).collect(),
            };
            match &mut self.tex {
                Some(t) => t.set(img, TextureOptions::NEAREST),
                None => self.tex = Some(ctx.load_texture("preview", img, TextureOptions::NEAREST)),
            }
        }
        let cfg = self.cfg.clone();

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(t) = &self.tex {
                    ui.image(egui::load::SizedTexture::new(t.id(), [384.0, 192.0]));
                }
                ui.weak(&status);
                ui.horizontal(|ui| {
                    if ui.button("◀ Page").clicked() {
                        send("PAGE -1");
                    }
                    if ui.button("Page ▶").clicked() {
                        send("PAGE 1");
                    }
                    let mut p = paused;
                    if ui.checkbox(&mut p, "SteelSeries screen").changed() {
                        send(if p { "PAUSE 1" } else { "PAUSE 0" });
                    }
                });
            });
            ui.add_space(6.0);

            ui.columns(2, |cols| {
                cols[0].group(|ui| {
                    ui.strong("Pages");
                    for (p, label) in PAGE_NAMES {
                        let mut on = cfg.pages.iter().any(|x| x == p);
                        if ui.checkbox(&mut on, *label).changed() {
                            let mut pages: Vec<&str> =
                                PAGES.iter().copied().filter(|q| if q == p { on } else { cfg.pages.iter().any(|x| x == q) }).collect();
                            if pages.is_empty() {
                                pages.push("clock");
                            }
                            set("pages", json!(pages));
                        }
                    }
                    ui.horizontal(|ui| {
                        ui.label("Rotate every");
                        number(ui, "rotate_seconds", cfg.rotate_seconds, 0, 3600);
                        ui.label("s (0 = off)");
                    });
                });
                cols[1].group(|ui| {
                    ui.strong("Visualizer");
                    let opts: Vec<(String, String)> =
                        std::iter::once("auto").chain(STYLES.iter().copied()).map(|s| (s.to_string(), s.to_string())).collect();
                    if let Some(v) = combo(ui, "viz", &cfg.visualizer, &opts, 140.0) {
                        set("visualizer", v);
                    }
                    ui.horizontal(|ui| {
                        ui.label("Auto: switch every");
                        number(ui, "visualizer_auto_seconds", cfg.visualizer_auto_seconds, 5, 600);
                        ui.label("s");
                    });
                    check(ui, "visualizer_peaks", cfg.visualizer_peaks, "Peak markers");
                    check(ui, "auto_music", cfg.auto_music, "Show music while playing");
                });
            });

            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.strong("Pioneer clips");
                egui::Grid::new("clips").num_columns(2).show(ui, |ui| {
                    ui.label("Clip");
                    let opts: Vec<(String, String)> = std::iter::once(("auto".to_string(), "auto (shuffle)".to_string()))
                        .chain(self.clips.iter().map(|n| (n.clone(), clip_label(n))))
                        .collect();
                    if let Some(v) = combo(ui, "clip", &cfg.clip, &opts, 220.0) {
                        set("clip", v);
                    }
                    ui.end_row();
                    ui.label("Fit");
                    ui.horizontal(|ui| {
                        let opts: Vec<(String, String)> =
                            ["fill", "fit", "stretch"].iter().map(|s| (s.to_string(), s.to_string())).collect();
                        if let Some(v) = combo(ui, "fit", &cfg.clip_fit, &opts, 80.0) {
                            set("clip_fit", v);
                        }
                        ui.label("Speed");
                        number(ui, "clip_fps", cfg.clip_fps, 2, 30);
                        ui.label("fps");
                    });
                    ui.end_row();
                    ui.label("Auto");
                    ui.horizontal(|ui| {
                        ui.label("new clip every");
                        number(ui, "clip_auto_seconds", cfg.clip_auto_seconds, 5, 3600);
                        ui.label("s");
                        check(ui, "clip_react", cfg.clip_react, "Faster with louder music");
                    });
                    ui.end_row();
                });
            });

            ui.group(|ui| {
                ui.set_width(ui.available_width());
                ui.strong("Display");
                egui::Grid::new("display").num_columns(2).show(ui, |ui| {
                    ui.label("Brightness");
                    let mut b = cfg.brightness;
                    if ui.add(egui::Slider::new(&mut b, 1..=10)).changed() {
                        set("brightness", b);
                    }
                    ui.end_row();
                    check(ui, "clock_24h", cfg.clock_24h, "24-hour clock");
                    check(ui, "clock_seconds_bar", cfg.clock_seconds_bar, "Clock seconds bar");
                    ui.end_row();
                    ui.label("Burn-in shift every");
                    ui.horizontal(|ui| {
                        number(ui, "pixel_shift_minutes", cfg.pixel_shift_minutes, 0, 60);
                        ui.label("min (0 = off)");
                    });
                    ui.end_row();
                    ui.label("Audio source");
                    let opts: Vec<(String, String)> = std::iter::once((String::new(), "(default output)".to_string()))
                        .chain(self.devices.iter().map(|d| (d.clone(), d.clone())))
                        .collect();
                    if let Some(v) = combo(ui, "dev", &cfg.audio_device, &opts, 240.0) {
                        set("audio_device", v);
                    }
                    ui.end_row();
                    ui.label("Font");
                    let opts: Vec<(String, String)> = fonts().iter().map(|f| (f.key.clone(), f.label.clone())).collect();
                    if let Some(v) = combo(ui, "font", &cfg.font, &opts, 240.0) {
                        set("font", v);
                    }
                    ui.end_row();
                    ui.label("Big clock");
                    let opts = vec![
                        ("classic".to_string(), "Classic".to_string()),
                        ("pixel".to_string(), "Pixel (uses the font)".to_string()),
                    ];
                    if let Some(v) = combo(ui, "clockfont", &cfg.clock_font, &opts, 240.0) {
                        set("clock_font", v);
                    }
                    ui.end_row();
                });
            });

            ui.horizontal(|ui| {
                if ui.checkbox(&mut self.autostart, "Start with Windows").changed() {
                    set_autostart(self.autostart);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if ui.button("Open config.json").clicked() {
                        let _ = std::process::Command::new("cmd")
                            .args(["/c", "start", "", &config::config_path().display().to_string()])
                            .spawn();
                    }
                });
            });
        });
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
}

pub fn run() -> anyhow::Result<()> {
    let feed = Arc::new(Mutex::new(Feed::default()));
    let f = feed.clone();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let mut g = f.lock().unwrap();
            if let Some(hex) = line.strip_prefix("FRAME ") {
                g.frame = hex
                    .chars()
                    .flat_map(|c| {
                        let n = c.to_digit(16).unwrap_or(0);
                        (0..4).rev().map(move |i| n >> i & 1 == 1)
                    })
                    .collect();
            } else if let Some(s) = line.strip_prefix("STATUS ") {
                g.status = s.to_string();
            } else if let Some(p) = line.strip_prefix("PAUSED ") {
                g.paused = p == "1";
            } else if line == "RAISE" {
                g.raise = true;
                if let Some(c) = &g.ctx {
                    c.request_repaint();
                }
            }
        }
        // the tray process went away: close too
        if let Some(c) = &f.lock().unwrap().ctx {
            c.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    let app = App {
        cfg: config::load(None),
        mtime: config::mtime(),
        feed,
        tex: None,
        clips: crate::lkd::ClipPlayer::new(config::data_dir().join("clips")).names,
        devices: crate::audio::output_devices(),
        autostart: autostart_enabled(),
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("nova-dock")
            .with_inner_size([440.0, 720.0])
            .with_resizable(false)
            .with_icon(egui::IconData { rgba: icon_rgba(true), width: 32, height: 32 }),
        ..Default::default()
    };
    eframe::run_native("nova-dock", options, Box::new(|_| Ok(Box::new(app)))).map_err(|e| anyhow::anyhow!("{e}"))
}
