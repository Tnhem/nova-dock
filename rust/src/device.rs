//! Arctis Nova Pro base station over USB HID (protocol from JerwuQu/ggoled).
//!
//! Draw: feature report on usage page 0xFFC0 of interface 4:
//!   [0x06, 0x93, x, y, w, h, bitmap...] padded to 1024 bytes, w <= 64, column-major, 8 rows per byte LSB = top.
//! 0x85 = brightness (1-10), 0x95 = back to the stock UI. Events (report 0x07) arrive on usage page 0xFF00.

use crate::canvas::{Canvas, H, W};
use hidapi::{HidApi, HidDevice};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const VID: u16 = 0x1038;
const PIDS: &[u16] = &[0x12CB, 0x12CD, 0x12E0, 0x12E5, 0x225D]; // Nova Pro wired/wireless, PC/Xbox

pub struct Dock {
    dev: HidDevice,
    pub oled_path: CString,
    pub info_path: Option<CString>,
    last: Option<Vec<bool>>,
}

impl Dock {
    pub fn connect() -> anyhow::Result<Dock> {
        let api = HidApi::new()?;
        let (mut oled, mut info) = (None, None);
        for d in api.device_list() {
            if d.vendor_id() == VID && PIDS.contains(&d.product_id()) && d.interface_number() == 4 {
                match d.usage_page() {
                    0xFFC0 => oled = Some(d.path().to_owned()),
                    0xFF00 => info = Some(d.path().to_owned()),
                    _ => {}
                }
            }
        }
        let oled = oled.ok_or_else(|| anyhow::anyhow!("Arctis Nova Pro base station not found"))?;
        let dev = api.open_path(&oled)?;
        Ok(Dock { dev, oled_path: oled, info_path: info, last: None })
    }

    /// Identical frames are not re-sent.
    pub fn draw(&mut self, frame: &Canvas) -> anyhow::Result<()> {
        if self.last.as_ref() == Some(&frame.px) {
            return Ok(());
        }
        self.last = Some(frame.px.clone());
        for x0 in [0, 64] {
            let mut buf = [0u8; 1024];
            buf[..6].copy_from_slice(&[0x06, 0x93, x0 as u8, 0, 64, 64]);
            for x in 0..64 {
                for y in 0..H {
                    if frame.get(x0 + x, y) {
                        buf[6 + (x * 8 + y / 8) as usize] |= 1 << (y % 8);
                    }
                }
            }
            self.feature(&buf)?;
        }
        Ok(())
    }

    fn feature(&self, buf: &[u8]) -> anyhow::Result<()> {
        let mut i = 0u64;
        loop {
            match self.dev.send_feature_report(buf) {
                Ok(()) => return Ok(()),
                Err(e) if i >= 10 => return Err(e.into()),
                Err(_) => {
                    i += 1;
                    std::thread::sleep(Duration::from_millis(i * i));
                }
            }
        }
    }

    fn command(&self, bytes: &[u8]) -> anyhow::Result<()> {
        let mut buf = [0u8; 64];
        buf[..bytes.len()].copy_from_slice(bytes);
        self.dev.write(&buf)?;
        Ok(())
    }

    pub fn brightness(&self, level: u8) -> anyhow::Result<()> {
        self.command(&[0x06, 0x85, level.clamp(1, 10)])
    }

    pub fn stock_ui(&mut self) -> anyhow::Result<()> {
        self.last = None;
        self.command(&[0x06, 0x95])
    }
}

impl Drop for Dock {
    fn drop(&mut self) {
        let _ = self.stock_ui();
    }
}

#[derive(Default, Clone)]
pub struct Status {
    pub volume: Option<f32>,
    pub vol_t: Option<Instant>,
    pub battery: Option<u8>,
    pub spare: Option<u8>,
    pub connected: Option<bool>,
}

impl Status {
    pub fn volume_overlay(&self) -> bool {
        self.volume.is_some() && self.vol_t.is_some_and(|t| t.elapsed() < Duration::from_millis(1500))
    }
}

/// Volume knob / battery / headset connection, read from the info collection in a thread.
pub struct DockStatus {
    pub state: Arc<Mutex<Status>>,
    alive: Arc<AtomicBool>,
}

impl DockStatus {
    pub fn start(dock: &Dock) -> DockStatus {
        let state = Arc::new(Mutex::new(Status::default()));
        let alive = Arc::new(AtomicBool::new(true));
        let (st, al) = (state.clone(), alive.clone());
        let (info, ctl) = (dock.info_path.clone(), dock.oled_path.clone());
        std::thread::spawn(move || {
            const VOL_MAX: f32 = 0x38 as f32;
            while al.load(Ordering::Relaxed) {
                let run = || -> anyhow::Result<()> {
                    let api = HidApi::new()?;
                    let info = api.open_path(info.as_ref().ok_or_else(|| anyhow::anyhow!("no info collection"))?)?;
                    let ctl = api.open_path(&ctl)?;
                    info.set_blocking_mode(false)?;
                    ctl.set_blocking_mode(false)?;
                    let mut last_q: Option<Instant> = None;
                    let mut buf = [0u8; 64];
                    while al.load(Ordering::Relaxed) {
                        if last_q.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
                            let mut q = [0u8; 64];
                            q[..2].copy_from_slice(&[0x06, 0xB0]); // ask for battery/connection
                            ctl.write(&q)?;
                            last_q = Some(Instant::now());
                        }
                        let mut n = info.read(&mut buf)?;
                        if n == 0 {
                            n = ctl.read(&mut buf)?;
                        }
                        if n < 16 {
                            std::thread::sleep(Duration::from_millis(50));
                            continue;
                        }
                        let mut s = st.lock().unwrap();
                        match (buf[0], buf[1]) {
                            (0x07, 0x25) => {
                                s.volume = Some((VOL_MAX - buf[2] as f32).max(0.0) / VOL_MAX);
                                s.vol_t = Some(Instant::now());
                            }
                            (0x07, 0xB7) => {
                                s.battery = Some(buf[2]);
                                s.spare = Some(buf[3]);
                            }
                            (0x07, 0xB5) => {
                                s.connected = Some(buf[4] == 8);
                                last_q = None;
                            }
                            (0x06, 0xB0) => {
                                s.connected = Some(buf[15] == 8);
                                s.battery = Some(buf[6]);
                                s.spare = Some(buf[7]);
                            }
                            _ => {}
                        }
                    }
                    Ok(())
                };
                if run().is_err() {
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        });
        DockStatus { state, alive }
    }

    pub fn get(&self) -> Status {
        self.state.lock().unwrap().clone()
    }
}

impl Drop for DockStatus {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::Relaxed);
    }
}

/// Burn-in protection: move the picture by 0/1 px every `minutes`, without wrapping edges.
pub fn pixel_shift(frame: &Canvas, minutes: u32) -> Canvas {
    if minutes == 0 {
        return frame.clone();
    }
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let (dx, dy) = [(0, 0), (1, 0), (1, 1), (0, 1)][((secs / (minutes as u64 * 60)) % 4) as usize];
    if dx == 0 && dy == 0 {
        return frame.clone();
    }
    let mut out = Canvas::new(W, H);
    out.paste(&frame.crop(0, 0, W - dx, H - dy), dx, dy);
    out
}
