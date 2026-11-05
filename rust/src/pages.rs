//! Pages and overlays.

use crate::canvas::{Canvas, H, W};
use crate::config::Config;
use crate::device::Status;
use crate::lkd::ClipPlayer;
use crate::media::NowPlaying;
use crate::sensors::System;
use crate::text::{BigFont, Text};
use crate::viz::{Frame, Visualizer};
use chrono::Local;

pub const BOTTOM: i32 = H - 2; // baseline of bottom text rows (1 px spare for the burn-in shift)
pub const ANIMATED: &[&str] = &["music", "visualizer", "pioneer"]; // pages that use audio
pub const ALWAYS_MOVING: &[&str] = &["dolphin", "warp", "pioneer"]; // styles that animate in silence

pub struct Ctx<'a> {
    pub cfg: &'a Config,
    pub text: &'a Text,
    pub big: &'a BigFont,
    pub status: &'a Status,
    pub now: &'a NowPlaying,
    pub system: Option<&'a mut System>, // None until the system page is first shown
    pub viz: &'a mut Visualizer,
    pub clips: &'a mut ClipPlayer,
    pub frame: &'a Frame,
    pub viz_style: &'a str,
    pub t: f64,
}

pub fn draw_page(name: &str, c: &mut Canvas, ctx: &mut Ctx) {
    match name {
        "system" => page_system(c, ctx),
        "music" => page_music(c, ctx),
        "visualizer" => {
            let (f, s, t, title) = (ctx.frame, ctx.viz_style, ctx.t, ctx.now.title.clone());
            ctx.viz.draw(c, s, f, 0, H - 1, t, ctx.cfg, ctx.clips, &title)
        }
        "pioneer" => page_pioneer(c, ctx),
        _ => page_clock(c, ctx),
    }
}

fn battery_icon(c: &mut Canvas, x: i32, y: i32, level: Option<u8>) {
    c.rect(x, y, x + 9, y + 5);
    c.fill_rect(x + 10, y + 2, x + 10, y + 3, true);
    if let Some(l) = level.filter(|l| *l > 0) {
        c.fill_rect(x + 2, y + 2, x + 2 + (5.0 * l.min(8) as f32 / 8.0).round() as i32, y + 3, true);
    }
}

/// Draw s clipped to width with its top at y, scrolling it back and forth when too long.
fn scroll_text(c: &mut Canvas, text: &Text, x: i32, y: i32, s: &str, width: i32, t: f64) {
    let tw = text.width(s) as f64;
    let mut off = 0.0;
    if tw > width as f64 {
        // 20 px/s, 1.5 s pause at each end
        let span = tw - width as f64;
        let mv = span / 20.0;
        let p = t % (2.0 * mv + 3.0);
        off = if p < 1.5 {
            0.0
        } else if p < 1.5 + mv {
            (p - 1.5) * 20.0
        } else if p < 3.0 + mv {
            span
        } else {
            span - (p - 3.0 - mv) * 20.0
        };
    }
    let mut layer = Canvas::new(width, text.height(s) + 1);
    text.draw(&mut layer, -(off.round() as f32), 0, s);
    c.paste(&layer, x, y);
}

fn page_clock(c: &mut Canvas, ctx: &mut Ctx) {
    let text = ctx.text;
    let now = Local::now();
    let hhmm = if ctx.cfg.clock_24h {
        now.format("%H:%M").to_string()
    } else {
        now.format("%I:%M").to_string().trim_start_matches('0').to_string()
    };
    let date = now.format("%a %d %b").to_string();
    let date_top = BOTTOM - text.cap;
    text.draw_base(c, 0.0, BOTTOM, &date);
    let mut area = date_top - 3; // rows available for the big time
    if ctx.cfg.clock_seconds_bar {
        area -= 2;
        let sec = now.format("%S").to_string().parse::<f32>().unwrap_or(0.0);
        c.line(0, area + 1, (sec / 59.0 * (W - 1) as f32).round() as i32, area + 1);
    }
    let big = if ctx.cfg.clock_font == "pixel" {
        // the chosen pixel font scaled up by a whole number, so it stays crisp
        let small = text.render(&hhmm);
        let k = ((W - 2) / small.w).min(area / small.h).max(1);
        small.scale(k)
    } else {
        ctx.big.render(&hhmm)
    };
    c.overlay(&big, (W - big.w) / 2, ((area - big.h) / 2).max(0));
    let room = W as f32 - text.width(&date); // big fonts: skip the status rather than overlap the date
    if ctx.status.connected == Some(false) && room > text.width("off") + 4.0 {
        text.draw_base(c, W as f32 - text.width("off"), BOTTOM, "off");
    } else if ctx.status.battery.is_some() && room > 15.0 {
        battery_icon(c, W - 12, BOTTOM - text.cap / 2 - 3, ctx.status.battery);
    }
}

fn bar(c: &mut Canvas, x: i32, y: i32, w: i32, h: i32, frac: f32) {
    c.rect(x, y, x + w - 1, y + h - 1);
    let fill = ((w - 2) as f32 * frac.clamp(0.0, 1.0)).round() as i32;
    if fill > 0 {
        c.fill_rect(x + 1, y + 1, x + fill, y + h - 2, true);
    }
}

fn human(bps: f64) -> String {
    let mut v = bps;
    for unit in ["B", "K", "M", "G"] {
        if v < 1000.0 {
            return if unit == "B" || v >= 10.0 { format!("{v:.0}{unit}") } else { format!("{v:.1}{unit}") };
        }
        v /= 1024.0;
    }
    format!("{v:.0}T")
}

fn gb(n: u64) -> String {
    format!("{:.1}", n as f64 / (1u64 << 30) as f64)
}

/// Right-hand columns for a clock + temperature, longest first.
fn mhz_variants(mhz: Option<f64>, temp: Option<f64>) -> Vec<Vec<String>> {
    let t_long: Vec<String> = temp.map(|t| format!("{t:.0}\u{b0}C")).into_iter().collect();
    let t_short: Vec<String> = temp.map(|t| format!("{t:.0}\u{b0}")).into_iter().collect();
    let mut out = vec![];
    if let Some(m) = mhz.filter(|m| *m > 0.0) {
        out.push([vec![format!("{m:.0}MHz")], t_long.clone()].concat());
        out.push([vec![format!("{:.2}G", m / 1000.0)], t_short.clone()].concat());
        out.push([vec![format!("{:.1}G", m / 1000.0)], t_short.clone()].concat());
    }
    out.push(t_long);
    out.push(t_short);
    out.push(vec![]);
    out
}

/// Table of CPU / GPU / RAM / VRAM rows (label, %, details) with thin usage bars, then network.
/// Columns shrink to fit the chosen font; rows that don't fit are dropped, least important first.
fn page_system(c: &mut Canvas, ctx: &mut Ctx) {
    let text = ctx.text;
    let Some(s) = ctx.system.as_deref_mut() else { return };
    s.update();
    let cap = text.cap;
    let pitch = cap + 3; // text, 1 px gap, 1 px bar, 1 px gap
    let mut rows: Vec<(&str, f32, Vec<Vec<String>>)> = vec![("CPU", s.cpu, mhz_variants(s.cpu_mhz, s.cpu_temp))];
    if let Some(g) = &s.gpu {
        rows.push(("GPU", g.usage as f32, mhz_variants(Some(g.clock as f64), Some(g.temp as f64))));
    }
    rows.push((
        "RAM",
        s.ram,
        vec![vec![format!("{}/{}G", gb(s.ram_used), gb(s.ram_total))], vec![format!("{}G", gb(s.ram_used))], vec![]],
    ));
    if let Some(g) = &s.gpu {
        let pct = g.vram_used as f32 / g.vram_total.max(1) as f32 * 100.0;
        rows.push(("VRAM", pct, vec![
            vec![format!("{}/{}G", gb(g.vram_used), gb(g.vram_total))],
            vec![format!("{}G", gb(g.vram_used))],
            vec![],
        ]));
    }
    let fit = ((BOTTOM - cap - 2) / pitch) as usize; // rows that fit above the network line
    for drop in ["VRAM", "GPU", "RAM"] {
        if rows.len() <= fit {
            break;
        }
        rows.retain(|r| r.0 != drop);
    }
    rows.truncate(fit.max(1));

    let label_w = rows.iter().map(|r| text.width(r.0)).fold(0.0, f32::max);
    let pct_right = label_w + 4.0 + text.width("100%");
    let mut y = 0;
    for (label, pct, variants) in &rows {
        text.draw_base(c, 0.0, y + cap, label);
        let p = format!("{pct:.0}%");
        text.draw_base(c, pct_right - text.width(&p), y + cap, &p);
        for cols in variants {
            // first set of right-aligned columns that fits
            let widths: Vec<f32> = cols.iter().map(|s| text.width(s)).collect();
            let total: f32 = widths.iter().sum::<f32>() + 5.0 * cols.len().saturating_sub(1) as f32;
            if W as f32 - total >= pct_right + 5.0 {
                let mut x = W as f32;
                for (s, w) in cols.iter().zip(&widths).rev() {
                    text.draw_base(c, x - w, y + cap, s);
                    x -= w + 5.0;
                }
                break;
            }
        }
        let by = y + cap + 1; // usage bar: dotted track, solid fill
        let fill = (pct.clamp(0.0, 100.0) / 100.0 * (W - 1) as f32).round() as i32;
        c.line(0, by, fill, by);
        let mut x = fill + 2;
        while x < W {
            c.on(x, by);
            x += 3;
        }
        y += pitch;
    }

    // spare room (small fonts): CPU history graph
    let bot = BOTTOM - cap - 3;
    if bot - y >= 6 {
        for (x, v) in s.hist.iter().enumerate() {
            let h = (v / 100.0 * (bot - y) as f32).round() as i32;
            if h > 0 {
                c.line(x as i32, bot - h, x as i32, bot);
            }
        }
    }
    let net = format!("\u{2193}{} \u{2191}{}", human(s.down), human(s.up));
    text.draw_base(c, 0.0, BOTTOM, &net);
    let clock = Local::now().format("%H:%M").to_string();
    if text.width(&net) + text.width(&clock) + 6.0 <= W as f32 {
        text.draw_base(c, W as f32 - text.width(&clock), BOTTOM, &clock);
    }
}

fn page_music(c: &mut Canvas, ctx: &mut Ctx) {
    let text = ctx.text;
    let np = ctx.now;
    let batt = ctx.status.battery.is_some();
    let mut top;
    if !np.title.is_empty() {
        scroll_text(c, text, 0, 0, &np.title, if batt { W - 13 } else { W }, ctx.t);
        top = text.height(&np.title) + 2;
        if !np.artist.is_empty() {
            scroll_text(c, text, 0, top, &np.artist, W, ctx.t + 1.0);
            top += text.height(&np.artist) + 2;
        }
    } else {
        text.draw(c, 0.0, 0, &Local::now().format("%H:%M").to_string());
        top = text.cap + 3;
    }
    if batt {
        battery_icon(c, W - 11, ((text.cap - 6) / 2).max(0), ctx.status.battery);
    }
    let (f, s, t, title) = (ctx.frame, ctx.viz_style, ctx.t, np.title.clone());
    ctx.viz.draw(c, s, f, top.min(H - 16), H - 1, t, ctx.cfg, ctx.clips, &title);
}

fn page_pioneer(c: &mut Canvas, ctx: &mut Ctx) {
    let f = ctx.frame;
    if !ctx.clips.draw(c, 0, H - 1, ctx.t, ctx.cfg, &ctx.now.title, f.loud, f.playing) {
        ctx.text.draw(c, 0.0, 18, "No clips in");
        ctx.text.draw(c, 0.0, 20 + ctx.text.cap + 4, "nova-dock/clips");
    }
}

/// Centred box with a label and, optionally, a percentage bar.
pub fn overlay_box(c: &mut Canvas, text: &Text, label: &str, value: Option<f32>) {
    let cap = text.cap;
    let inner = cap + if value.is_some() { 10 } else { 0 };
    let y0 = (H - inner) / 2 - 4;
    c.fill_rect(4, y0, W - 5, y0 + inner + 7, false);
    c.rect(4, y0, W - 5, y0 + inner + 7);
    text.draw(c, 8.0, y0 + 4, label);
    if let Some(v) = value {
        let pct = format!("{:.0}%", v * 100.0);
        text.draw(c, W as f32 - 8.0 - text.width(&pct), y0 + 4, &pct);
        bar(c, 8, y0 + cap + 7, W - 16, 6, v);
    }
}
