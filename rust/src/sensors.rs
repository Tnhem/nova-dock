//! CPU / GPU / RAM / network readings without admin rights.
//!
//! CPU clock: PDH "% Processor Performance" x base clock. GPU: NVML (ships with the NVIDIA driver).
//! CPU temperature: from a running HWiNFO (shared memory) or LibreHardwareMonitor (web server).

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::Performance::*;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

#[derive(Clone, Default)]
pub struct Gpu {
    pub usage: u32,
    pub clock: u32,
    pub temp: u32,
    pub vram_used: u64,
    pub vram_total: u64,
}

struct CpuClock {
    base: f64,
    query: PDH_HQUERY,
    counter: PDH_HCOUNTER,
    ok: bool,
}

impl CpuClock {
    fn new() -> CpuClock {
        let base = winreg::RegKey::predef(winreg::enums::HKEY_LOCAL_MACHINE)
            .open_subkey(r"HARDWARE\DESCRIPTION\System\CentralProcessor\0")
            .and_then(|k| k.get_value::<u32, _>("~MHz"))
            .unwrap_or(0) as f64;
        let mut query = PDH_HQUERY::default();
        let mut counter = PDH_HCOUNTER::default();
        let ok = unsafe {
            PdhOpenQueryW(None, 0, &mut query) == 0
                && PdhAddEnglishCounterW(query, w!(r"\Processor Information(_Total)\% Processor Performance"), 0, &mut counter) == 0
        };
        if ok {
            unsafe { PdhCollectQueryData(query) };
        }
        CpuClock { base, query, counter, ok }
    }

    fn mhz(&self) -> Option<f64> {
        if !self.ok || self.base == 0.0 {
            return None;
        }
        unsafe {
            PdhCollectQueryData(self.query);
            let mut v = PDH_FMT_COUNTERVALUE::default();
            (PdhGetFormattedCounterValue(self.counter, PDH_FMT_DOUBLE, None, &mut v) == 0)
                .then(|| self.base * v.Anonymous.doubleValue / 100.0)
        }
    }
}

struct Nvml {
    _lib: Option<libloading::Library>,
    dev: *mut std::ffi::c_void,
    util: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut [u32; 2]) -> i32>,
    clock: Option<unsafe extern "C" fn(*mut std::ffi::c_void, u32, *mut u32) -> i32>,
    temp: Option<unsafe extern "C" fn(*mut std::ffi::c_void, u32, *mut u32) -> i32>,
    mem: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *mut [u64; 3]) -> i32>,
}

impl Nvml {
    fn new() -> Nvml {
        let mut n = Nvml { _lib: None, dev: std::ptr::null_mut(), util: None, clock: None, temp: None, mem: None };
        unsafe {
            let Ok(lib) = libloading::Library::new("nvml.dll") else { return n };
            let init: libloading::Symbol<unsafe extern "C" fn() -> i32> = match lib.get(b"nvmlInit_v2\0") {
                Ok(s) => s,
                Err(_) => return n,
            };
            if init() != 0 {
                return n;
            }
            let Ok(by_index) = lib.get::<unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32>(b"nvmlDeviceGetHandleByIndex_v2\0")
            else {
                return n;
            };
            if by_index(0, &mut n.dev) != 0 {
                return n;
            }
            n.util = lib.get(b"nvmlDeviceGetUtilizationRates\0").ok().map(|s| *s);
            n.clock = lib.get(b"nvmlDeviceGetClockInfo\0").ok().map(|s| *s);
            n.temp = lib.get(b"nvmlDeviceGetTemperature\0").ok().map(|s| *s);
            n.mem = lib.get(b"nvmlDeviceGetMemoryInfo\0").ok().map(|s| *s);
            n._lib = Some(lib);
        }
        n
    }

    fn read(&self) -> Option<Gpu> {
        if self.dev.is_null() {
            return None;
        }
        unsafe {
            let mut u = [0u32; 2];
            if self.util?(self.dev, &mut u) != 0 {
                return None;
            }
            let (mut clk, mut t, mut m) = (0u32, 0u32, [0u64; 3]);
            if let Some(f) = self.clock {
                f(self.dev, 0, &mut clk); // NVML_CLOCK_GRAPHICS
            }
            if let Some(f) = self.temp {
                f(self.dev, 0, &mut t); // NVML_TEMPERATURE_GPU
            }
            if let Some(f) = self.mem {
                f(self.dev, &mut m); // total, free, used
            }
            Some(Gpu { usage: u[0], clock: clk, temp: t, vram_used: m[2], vram_total: m[0] })
        }
    }
}

impl Drop for Nvml {
    fn drop(&mut self) {
        if let (Some(lib), false) = (&self._lib, self.dev.is_null()) {
            unsafe {
                if let Ok(shutdown) = lib.get::<unsafe extern "C" fn() -> i32>(b"nvmlShutdown\0") {
                    shutdown();
                }
            }
        }
    }
}

const TEMP_LABELS: &[&str] = &[
    "CPU (Tctl/Tdie)", "Core (Tctl/Tdie)", "CPU Die (average)", "CPU Package", "Core (Tctl)", "CPU Tctl/Tdie",
    "Core Average", "CPU",
];

fn hwinfo_temp() -> Option<f64> {
    use windows::Win32::System::Memory::*;
    unsafe {
        let h = OpenFileMappingW(FILE_MAP_READ.0, false, w!("Global\\HWiNFO_SENS_SM2")).ok()?;
        let view = MapViewOfFile(h, FILE_MAP_READ, 0, 0, 0);
        let _ = windows::Win32::Foundation::CloseHandle(h);
        let base = view.Value as *const u8;
        if base.is_null() {
            return None;
        }
        let rd = |off: usize| u32::from_le_bytes(std::slice::from_raw_parts(base.add(off), 4).try_into().unwrap());
        let result = (|| {
            if rd(0) != 0x53695748 {
                return None; // "HWiS"
            }
            let (off_r, size_r, num_r) = (rd(32) as usize, rd(36) as usize, rd(40) as usize);
            let mut found: Vec<(String, f64)> = vec![];
            for i in 0..num_r {
                let rec = std::slice::from_raw_parts(base.add(off_r + i * size_r), 324.min(size_r));
                if u32::from_le_bytes(rec[0..4].try_into().unwrap()) != 1 {
                    continue; // SENSOR_TYPE_TEMP
                }
                let label = &rec[12..140];
                let label = String::from_utf8_lossy(&label[..label.iter().position(|b| *b == 0).unwrap_or(128)]).to_string();
                found.push((label, f64::from_le_bytes(rec[316..324].try_into().unwrap())));
            }
            TEMP_LABELS.iter().find_map(|l| found.iter().find(|(n, _)| n == l).map(|(_, v)| *v))
        })();
        let _ = UnmapViewOfFile(view);
        result
    }
}

fn lhm_temp() -> Option<f64> {
    let addr = "127.0.0.1:8085".parse().ok()?;
    let mut s = std::net::TcpStream::connect_timeout(&addr, Duration::from_millis(300)).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    s.write_all(b"GET /data.json HTTP/1.0\r\nHost: localhost\r\n\r\n").ok()?;
    let mut body = String::new();
    s.read_to_string(&mut body).ok()?;
    let json: serde_json::Value = serde_json::from_str(body.split("\r\n\r\n").nth(1)?).ok()?;
    let mut found: Vec<(String, f64)> = vec![];
    fn walk(n: &serde_json::Value, in_cpu: bool, found: &mut Vec<(String, f64)>) {
        let text = n["Text"].as_str().unwrap_or("");
        let in_cpu = in_cpu || ["Ryzen", "Intel", "AMD", "CPU"].iter().any(|k| text.contains(k));
        if let Some(v) = n["Value"].as_str() {
            if in_cpu && v.contains("°C") {
                if let Ok(x) = v.split_whitespace().next().unwrap_or("").replace(',', ".").parse() {
                    found.push((text.to_string(), x));
                }
            }
        }
        for c in n["Children"].as_array().into_iter().flatten() {
            walk(c, in_cpu, found);
        }
    }
    walk(&json, false, &mut found);
    TEMP_LABELS.iter().find_map(|l| found.iter().find(|(n, _)| n == l).map(|(_, v)| *v))
}

/// CPU temperature polled in the background (only while someone keeps asking).
struct CpuTemp {
    value: Arc<Mutex<(Option<f64>, Instant)>>,
}

impl CpuTemp {
    fn new() -> CpuTemp {
        let value = Arc::new(Mutex::new((None, Instant::now())));
        let v = value.clone();
        std::thread::spawn(move || loop {
            if Arc::strong_count(&v) == 1 {
                break; // System was dropped
            }
            let wanted = v.lock().unwrap().1.elapsed() < Duration::from_secs(5);
            if wanted {
                let t = hwinfo_temp().or_else(lhm_temp);
                v.lock().unwrap().0 = t;
                std::thread::sleep(Duration::from_secs(if t.is_some() { 2 } else { 10 }));
            } else {
                std::thread::sleep(Duration::from_secs(1));
            }
        });
        CpuTemp { value }
    }

    fn read(&self) -> Option<f64> {
        let mut v = self.value.lock().unwrap();
        v.1 = Instant::now();
        v.0
    }
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

fn net_totals() -> (u64, u64) {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return (0, 0);
        }
        let rows = std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let mut sum = (0u64, 0u64);
        for r in rows {
            // physical / hardware interfaces only, so virtual adapters don't double count
            if r.InterfaceAndOperStatusFlags._bitfield & 1 != 0 {
                sum.0 += r.InOctets;
                sum.1 += r.OutOctets;
            }
        }
        FreeMibTable(table as *const _);
        sum
    }
}

/// CPU / GPU / RAM / network, sampled at most once a second (only while the system page is shown).
pub struct System {
    pub hist: Vec<f32>,
    pub cpu: f32,
    pub ram: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub cpu_mhz: Option<f64>,
    pub cpu_temp: Option<f64>,
    pub gpu: Option<Gpu>,
    pub down: f64,
    pub up: f64,
    t: Option<Instant>,
    times: (u64, u64),
    net: (u64, u64, Instant),
    clock: CpuClock,
    nvml: Nvml,
    temp: CpuTemp,
}

impl System {
    pub fn new() -> System {
        let (n_in, n_out) = net_totals();
        let mut s = System {
            hist: vec![0.0; 128],
            cpu: 0.0,
            ram: 0.0,
            ram_used: 0,
            ram_total: 0,
            cpu_mhz: None,
            cpu_temp: None,
            gpu: None,
            down: 0.0,
            up: 0.0,
            t: None,
            times: (0, 0),
            net: (n_in, n_out, Instant::now()),
            clock: CpuClock::new(),
            nvml: Nvml::new(),
            temp: CpuTemp::new(),
        };
        s.times = s.cpu_times();
        s
    }

    fn cpu_times(&self) -> (u64, u64) {
        let (mut idle, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe {
            let _ = GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user));
        }
        (ft(idle), ft(kernel) + ft(user)) // kernel time includes idle
    }

    pub fn update(&mut self) {
        if self.t.is_some_and(|t| t.elapsed() < Duration::from_secs(1)) {
            return;
        }
        self.t = Some(Instant::now());
        let (idle, total) = self.cpu_times();
        let (di, dt) = (idle - self.times.0, total - self.times.1);
        self.cpu = if dt > 0 { (100.0 * (1.0 - di as f64 / dt as f64)) as f32 } else { 0.0 };
        self.times = (idle, total);
        self.hist.remove(0);
        self.hist.push(self.cpu);
        let mut ms = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut ms) }.is_ok() {
            self.ram_total = ms.ullTotalPhys;
            self.ram_used = ms.ullTotalPhys - ms.ullAvailPhys;
            self.ram = self.ram_used as f32 / self.ram_total.max(1) as f32 * 100.0;
        }
        self.cpu_mhz = self.clock.mhz();
        self.cpu_temp = self.temp.read();
        self.gpu = self.nvml.read();
        let (n_in, n_out) = net_totals();
        let secs = self.net.2.elapsed().as_secs_f64().max(1e-3);
        self.down = n_in.saturating_sub(self.net.0) as f64 / secs;
        self.up = n_out.saturating_sub(self.net.1) as f64 / secs;
        self.net = (n_in, n_out, Instant::now());
    }
}
