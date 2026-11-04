# Lowtide

A tiny Windows tray app that tells you when your wireless mouse (or other
peripheral) is running out of battery, before it dies on you.

When a device drops below a threshold, Lowtide shows a **click-through
overlay** in a corner of your primary monitor. The overlay never takes focus,
never shows up in Alt-Tab, and your mouse passes straight through it. Press a
hotkey (default `Ctrl+Alt+Shift+B`) to see a short status popup with charge,
charging state, connection and firmware.

- One self-contained `lowtide.exe` (~600 KB), with no installer or runtime.
- Idle cost is effectively zero: about 12 MB working set, 2 threads, and no
  timers or polling loops between checks.
- Battery events are pushed by the device and handled immediately. A
  fallback check runs every 60 s (configurable).

## Supported devices

Lowtide talks Logitech **HID++ 2.0** directly over USB. It does not keep a
product-ID list: it probes every Logitech HID++ interface, so it works over a
receiver (LIGHTSPEED, Unifying, Bolt) and over a cable alike.

| Device | Battery feature | Notes |
|---|---|---|
| Logitech PRO 2 LIGHTSPEED | 0x1004 Unified Battery | Percentage reported by the mouse. Verified on hardware. |
| Logitech G903 LIGHTSPEED / G903 HERO | 0x1001 Battery Voltage | Percentage estimated from voltage with a Li-ion discharge curve. Covered by a simulator; not yet tested on a real G903. |
| Other HID++ 2.0 devices with 0x1000 / 0x1001 / 0x1004 | | Should work as well. |

A mouse that is cabled and paired to a receiver at the same time is shown
once, as wired. The two paths are matched by the device's unit ID.

## Usage

Run `lowtide.exe`. It lives in the notification area:

- **Left-click** the tray icon, or press the hotkey, to show the status popup.
- **Right-click** for *Show battery status*, *Refresh now*, *Settings…* and *Quit*.
- The tray icon shows the lowest charge. It is blue normally, green while
  charging and red in alert.

The low-battery alert and the status popup are independent. The popup never
hides an active alert.

### Settings

*Settings…* (or `lowtide.exe --settings`) opens a native dialog for:

- threshold
- whether to alert while charging
- check interval
- hotkey
- **Start with Windows**
- for each overlay: position (9 anchors), margin or offset, popup duration,
  font, text and background colour, opacity, padding and corner radius

**Preview** shows both overlays with the unsaved settings.

All percentages are 0–100. The settings live in
`%APPDATA%\lowtide\config.toml`, which you can also edit by hand:

```toml
threshold_percent = 15
suppress_while_charging = true
hotkey = "Ctrl+Alt+Shift+B"   # empty disables it; Win is allowed here
poll_interval_s = 60

[alert]
message = "{device} battery low: {percent}%"   # also {status}
anchor = "bottom_right"   # top_left, top, top_right, left, center, right, bottom_left, bottom
margin = 24

[alert.style]
font_family = "Segoe UI"
size_pt = 20.0
weight = 600
italic = false
text_color = "#FFFFFF"
text_opacity = 100
background = "#B00020"
background_opacity = 80
padding = 12
corner_radius = 8

[info]
anchor = "center"
margin = 24
offset = [0, 0]
duration_ms = 2500
# [info.style] takes the same keys as [alert.style]
```

### Command line

| Flag | Effect |
|---|---|
| `--probe` | Scan once, print all devices as JSON, and exit. Use this to check new hardware. |
| `--watch [seconds]` | Run the monitor and print every update (events and polls) live. |
| `--simulate N` | Use a fake device at N % to try out overlays and settings. |
| `--settings` | Open the settings dialog (of the running instance, if there is one). |

Starting `lowtide.exe` while it is already running shows the status popup of
the running instance.

## How it works

- `src/sources/logitech_hidpp`: HID++ over Win32 overlapped I/O.
  - Each HID handle keeps one read armed. The monitor thread sleeps in a
    single `WaitForMultipleObjects` on those reads plus a wake event, so
    battery events (feature event 0) and receiver link notifications (0x41)
    are handled the moment they arrive, without polling.
  - Receivers answer "no device in this slot" with HID++ 1.0 short reports.
    Windows delivers these only to the short collection, so requests listen
    on both collections.
- `src/monitor.rs`: the source-agnostic loop. It runs scheduled polls,
  rescans after plug and unplug (rate-limited), and publishes snapshots.
- `src/ui`: a hidden top-level window (tray, hotkey, device and display
  changes) plus two layered overlay windows.
  - The overlays are drawn with Direct2D (software target) and DirectWrite
    into a premultiplied DIB, then handed to the compositor with
    `UpdateLayeredWindow`. They are redrawn only when their content changes.
  - Alerts use 2 points of hysteresis so voltage noise can't make them flicker.

### Adding another vendor

The core only knows the vendor-neutral types in `src/model.rs`. To add a
device family:

1. Implement `sources::BatterySource` (`rescan`, `poll`, `wait_handles`,
   `process_pending`, `needs_rescan`, `devices`) in a new module under
   `src/sources/`.
2. Add it to `sources::all()`.

The UI, alert logic and config need no changes.

## Limitations

- Overlays can't draw over games running in *exclusive* fullscreen.
  Borderless/windowed fullscreen works.
- 0x1001 devices (G903) report voltage, not charge, so their percentage is an
  estimate. It is shown with a `~` in the status popup.

## Building

Requires Rust (MSVC toolchain) and the Windows SDK, for `rc.exe`.

```
cargo test
cargo build --release   # -> target\release\lowtide.exe
```

The C runtime is linked statically (`.cargo/config.toml`), and the icon,
manifest and dialog are embedded, so the exe depends only on DLLs that ship
with Windows.

`tools/make_icon.py` (Pillow) regenerates `res/lowtide.ico`.
