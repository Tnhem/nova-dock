//! Now playing from Windows media controls (Spotify, browsers, ...), polled every 2 s in a thread.

use std::sync::{Arc, Mutex};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as Manager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

#[derive(Clone, Default)]
pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

pub fn start() -> Arc<Mutex<NowPlaying>> {
    let state = Arc::new(Mutex::new(NowPlaying::default()));
    let st = state.clone();
    std::thread::spawn(move || {
        unsafe {
            let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        }
        let Ok(mgr) = Manager::RequestAsync().and_then(|r| r.get()) else { return };
        loop {
            let np = (|| -> windows::core::Result<NowPlaying> {
                let s = mgr.GetCurrentSession()?;
                let p = s.TryGetMediaPropertiesAsync()?.get()?;
                Ok(NowPlaying {
                    title: p.Title()?.to_string_lossy(),
                    artist: p.Artist()?.to_string_lossy(),
                    playing: s.GetPlaybackInfo()?.PlaybackStatus()? == Status::Playing,
                })
            })()
            .unwrap_or_default();
            *st.lock().unwrap() = np;
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
    state
}
