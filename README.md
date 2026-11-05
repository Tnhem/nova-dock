# nova-dock

Custom screens for the **SteelSeries Arctis Nova Pro** base station OLED (128×64, 1-bit), written in Rust.
It talks to the dock directly over USB HID, so SteelSeries GG is not needed. It runs in the system tray and uses less than 1% of one CPU core.

## Download
Get **nova-dock-…-windows-x64.zip** from the [latest release](https://github.com/Tnhem/nova-dock/releases/latest), extract it anywhere and run `nova-dock.exe`. Keep the exe next to the `fonts` folder.

The exe isn't code-signed, so Windows SmartScreen may warn on first run (*More info → Run anyway*).

## Pages
- **clock**: large time, a seconds line, the date and the headset battery
- **system**: a table of rows with a thin usage bar under each, plus network speeds
  - CPU: usage, clock and temperature
  - GPU: usage, clock and temperature
  - RAM and VRAM: used / total
  - The details shorten to fit the chosen font
- **music**: the current track (Spotify, browser, etc. through Windows media controls) above a visualizer
- **visualizer**: a full-screen visualizer
- **pioneer**: plays original Pioneer car-stereo screen animations (`.lkd` clips, see below)

While audio is playing it switches to **music** automatically. When you turn the volume knob, a volume overlay pops up.

### Visualizer styles
- **bars**: 32 spectrum bars
- **segments**: hi-fi LED meter made of stacked blocks
- **mirror**: bars grow up and down from the centre, with bass in the middle
- **mountain**: a smooth spectrum outline with a dithered fill
- **scope**: the raw waveform, like an oscilloscope
- **waterfall**: a scrolling spectrogram
- **radial**: spokes around a ring that pulses with the bass
- **warp**: a starfield that speeds up with the music
- **dolphin**: a nod to the old Pioneer head units. A dolphin swims at night, jumps out of the waves on bass beats, and splashes and ripples the water
- **pioneer**: plays a Pioneer clip under the song title

`auto` switches style on every new track and every `visualizer_auto_seconds`.

## Build and run
Needs Windows and a Rust toolchain.

```
cd rust
cargo build --release
target\release\nova-dock.exe
```

- **Run modes:**
  - `nova-dock.exe` is the tray app.
  - `--no-tray` runs it in a console; Ctrl+C restores the stock screen.
  - `--preview out.pgm` renders every page, font and style to an image without using the dock.
- **Files:** the exe finds `config.json`, `fonts/` and `clips/` by walking up from its own folder, so running it from `rust/target/release` uses the repository root. Set `NOVA_DOCK_HOME` to use another folder.
- **Settings:** `config.json` is created on first run.

## Tray
- **Right-click** the icon to pick a page, a visualizer style, a Pioneer clip, a font or the brightness. The same menu toggles the SteelSeries screen and *Start with Windows*.
- **Double-click** the icon, or choose *Settings…*, to open a window with a live preview of the dock and every setting. The settings window is a separate process that only exists while it's open.
- Changes take effect immediately and are saved to `config.json`.
- **Quit** gives the screen back to SteelSeries.

## Hotkeys
| Keys | Action |
|---|---|
| Ctrl+Alt+→ / ← | next / previous page |
| Ctrl+Alt+↑ | next visualizer style (auto → bars → … → pioneer) |
| Ctrl+Alt+↓ | switch between the stock SteelSeries screen and this app |

## Fonts
Pick a font in the tray menu (*Font*), or in Settings. Each font is drawn at the size where it lands exactly on the pixel grid.

| Font | Notes |
|---|---|
| Pixel Operator | default |
| Minecraft (Monocraft) | Minecraft's 7 px letters |
| Minecraft large | ×2 |
| Press Start 2P | chunky arcade |
| Silkscreen | tiny caps |
| Unifont | also has Chinese, Japanese and Korean |

- Set *Big clock: Pixel* to draw the clock in the chosen font, scaled up 2–4× with crisp edges.
- Chinese, Japanese and Korean characters in song titles fall back to Unifont automatically.
- Font licences are in [`fonts/LICENSES.md`](fonts/LICENSES.md).

## System sensors
| Value | Where it comes from |
|---|---|
| CPU MHz | Windows performance counter (the *Speed* that Task Manager shows) |
| GPU usage, clock, temperature, VRAM | NVIDIA NVML, which comes with the driver |
| CPU temperature | HWiNFO or LibreHardwareMonitor, while one of them is running |

Nothing needs admin rights.

Windows itself can't read Ryzen or Intel core temperatures without a kernel driver, so the CPU temperature only shows up when one of these is running:
- **HWiNFO**, with *Shared Memory Support* turned on in its settings
- **LibreHardwareMonitor**, with *Options → Remote Web Server* turned on

## Pioneer clips
The **pioneer** page and visualizer style play `.lkd` animations from old Pioneer/Carrozzeria head units, such as the leaping dolphins.
- The clips are not included here, because the artwork is Pioneer's.
- [carozerra](https://github.com/youxufkhan/carozerra) worked out the `.lkd` format and collects the clips in `assets/clips`. Copy those `.lkd` files into a `clips/` folder next to `fonts/`.
- `movie8_f` is the leaping dolphins. `auto` shuffles through the animated clips.
- They play at 16 fps like the original, or faster with louder music if you turn that on.
- `fill` crops each clip to cover the 128×64 screen. `fit` shows the whole clip with black bars. `stretch` squeezes the whole clip onto the screen.
- The original screen's four tones (black, two shades of cyan, white) are dithered onto the 1-bit screen.

## Config (`config.json`, read again while the app runs)
- `pages`: the order of pages, chosen from `clock`, `system`, `music`, `visualizer` and `pioneer`
- `rotate_seconds`: cycles through the pages automatically (0 = off)
- `auto_music`: shows the music page while sound is playing
- `brightness`: 1–10
- `clock_24h`: 24-hour clock on or off
- `font`: `pixel-operator`, `minecraft`, `minecraft-large`, `press-start`, `silkscreen` or `unifont`
- `clock_font`: `classic` (Bahnschrift) or `pixel` (the chosen font, scaled up)
- `clock_seconds_bar`: the thin line under the clock that fills up over each minute
- `visualizer`: `auto` or one style name
- `visualizer_auto_seconds`: how often `auto` changes style
- `visualizer_peaks`: peak-hold markers above the bars (off by default)
- `clip`: `auto` or a clip name from `clips/`, without `.lkd`
- `clip_fit`: `fill`, `fit` or `stretch`
- `clip_fps`: playback speed in frames per second
- `clip_auto_seconds`: how often `auto` changes clip
- `clip_react`: play faster with louder music
- `audio_device`: part of the output device name to visualize (`""` = default output)
- `pixel_shift_minutes`: moves the picture by 1 px every N minutes to protect against burn-in

## Resource use
| | |
|---|---|
| Idle (clock page) | 23 MB (5 MB private), under 1% of one core |
| System page | 54 MB (33 MB private): NVIDIA's NVML library allocates about 28 MB once it starts, so it's only loaded the first time the system page is shown |
| Visualizer at 30 fps | a few % of one core |
| Settings window | its own process, only while open |

## How it works
The protocol comes from [JerwuQu/ggoled](https://github.com/JerwuQu/ggoled).
- **Drawing:** a HID *feature report* goes to interface 4, usage page `0xFFC0`. Its layout is `[0x06, 0x93, x, y, w, h, bitmap…]`, padded to 1024 bytes.
  - Each report holds at most 64 columns, so it takes two reports to fill the screen.
  - The bitmap is column-major. Each byte holds 8 vertical pixels, and the least significant bit is the top one.
- **Other commands:** `0x85 <1-10>` sets the brightness and `0x95` goes back to the stock UI. Both are sent as output reports.
- **Status:** the volume (`07 25`), battery (`07 b7`) and connection (`07 b5`) events arrive on usage page `0xFF00`. `06 b0` asks for the battery and connection state.
- **Keeping it light:** a frame identical to the last one is not sent. Static pages redraw twice a second. Only the animated pages run at 30 fps, and only while something is moving.
- **Audio:** captured with WASAPI loopback from the default output device, then a 4096-point FFT splits it into 64 log-spaced bands.
