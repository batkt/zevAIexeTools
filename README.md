# Zev Detector

Watches a Windows machine for activity and errors and reports what it finds to
udirdlagaBack, where the Zevtabs **Detector** page renders it.

Built to sit next to `dahua-service.exe` (dahuarust) on parking / kiosk PCs, but
nothing in it is Dahua-specific — point it at any service and any log file.

It ships as two binaries:

| | |
|-|-|
| `zev-detector.exe` | The agent. Runs as a Windows service; no window. This is the part that does the work. |
| `zev-detector-ui.exe` | The desktop app. Edits the config, shows the service state, installs / starts / stops it. |

They are separate so the service the SCM launches never loads a WebView, and so
closing the app never stops the detector.

## What it watches

| # | Source | What it catches |
|---|--------|-----------------|
| 1 | **Windows Event Log** | Critical / Error / Warning events on `System`, `Application`, or any other channel — service crashes, unexpected shutdowns, disk and driver faults, application faults. Messages are resolved through the publisher's own message table, so you get the real text and not `%1 %2`. |
| 2 | **Windows services** | Start / stop / crash transitions and exit codes for the services you list, read from the Service Control Manager. A service that stays down re-announces every 15 minutes. |
| 3 | **Application log files** | Tails `service.log` (dahuarust) or any other log, matching `[ERROR]`, `[WARN]`, `panicked`, `Aldaa` and friends. Handles rotation and remembers its byte offset across restarts. |
| 4 | **Host / endpoint health** | CPU, RAM and per-disk thresholds, plus HTTP probes (e.g. `http://127.0.0.1:5000/api/health` for the Dahua service). |

Everything is classified **Critical / Warning / Info**, de-duplicated by
fingerprint before it leaves the machine, queued to disk when the network is
down, and shipped in batches.

## Install

Run `Zev-Detector-Setup-1.0.0.exe` and click through it. It installs both
binaries into `C:\Program Files\Zev Detector` and adds Start Menu and Desktop
shortcuts. It does not touch the service — there is nothing to configure it
with yet.

Then open **Zev Detector** from the Start Menu. On a machine with no
`config.toml` the app opens a setup wizard: server URL and token, how this
machine should identify itself, what to watch, and finally a summary — the last
step writes the config, registers the `ZevDetector` service and starts it. One
UAC prompt, at that point and no earlier.

For silent or mass deployment, install with `/VERYSILENT`, drop a prepared
`config.toml` next to the exes, and run `zev-detector.exe install` yourself.

Uninstall through Apps & Features. It stops and deregisters the service, then
asks whether to delete `config.toml` too.

### Without the installer

Copy `zev-detector.exe`, `zev-detector-ui.exe` and `config.sample.toml` into a
folder and open `zev-detector-ui.exe` — the wizard does the rest. Everything
lives next to the exes, so the folder is self-contained.

## The desktop app

`zev-detector-ui.exe` runs **unelevated**. Reading the service state does not
need Administrator rights, so the window opens without a UAC prompt; only the
four buttons that change the service (install, uninstall, start, stop) ask for
elevation, one prompt per click, by re-running `zev-detector.exe <verb>`.

| Screen | |
|--------|--|
| Тойм | Service state and PID, the four service buttons, what is currently being watched, and a **Test run** that collects one pass and uploads nothing |
| Сервер | URL, token, timeouts, batch size, offline queue depth |
| Энэ машин | Agent name, organisation and building ids, heartbeat interval |
| Event Log | Channels, level, providers, and the ignore lists |
| Үйлчилгээ / Лог файл / Эрүүл мэнд | The repeating sections, added and removed inline |
| Бүртгэл | The tail of `zev-detector.log` |

Ctrl+S saves. The **Хадгалах** button is disabled until something actually
changes, so it doubles as an unsaved-changes indicator.

## Build

Needs a Rust toolchain (MSVC target). WebView2 is already present on Windows 11
and on any up-to-date Windows 10.

```
cargo build --release
```

Produces `target\release\zev-detector.exe` and
`target\release\zev-detector-ui.exe`.

To build the installer, install [Inno Setup 6](https://jrsoftware.org/isdl.php)
and then:

```
"C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\setup.iss
```

Output: `dist\Zev-Detector-Setup-<version>.exe`. The script reads the binaries
from `target\release`, so build them first.

### Working on the UI

The frontend is plain HTML/CSS/JS in `ui/` — no bundler, no npm. Serve that
folder and open it in a browser to iterate on the design without rebuilding
Rust:

```
python -m http.server 5175 --directory ui
```

`ui/mock.js` notices that `window.__TAURI__` is missing and answers every
command with fixture data; inside the real app it does nothing. Add `?fresh=1`
to the URL to preview the first-run wizard.

## Files it creates

| File | Purpose |
|------|---------|
| `config.toml` | Written by the app; the service reads it at startup |
| `zev-detector.log` | Its own log |
| `state.json` | Log-file read offsets, so restarts don't re-send old lines |
| `queue.json` | Events waiting for the server to come back |

Where those live depends on how it was installed:

| Layout | Location |
|--------|----------|
| Installed | `C:\ProgramData\Zev Detector\` |
| Portable | next to the exes |

The installed build has to split them out because the app runs unelevated and
`C:\Program Files` is not user-writable, while the service runs as LocalSystem —
ProgramData is the one directory both can use, and the installer grants Users
modify rights on it. A `config.toml` sitting next to the exes wins over
ProgramData, which is what keeps a copied folder (and `cargo run`)
self-contained.

## Command line

The service binary keeps its CLI, which is what the app and the installer drive:

```
zev-detector.exe install      :: register the service (needs Administrator)
zev-detector.exe uninstall    :: remove it
zev-detector.exe start|stop   :: control it
zev-detector.exe status       :: print its state
zev-detector.exe test         :: one collection pass, uploads nothing
zev-detector.exe run          :: run in the console (Ctrl+C to quit)
```

The service runs as **LocalSystem**, which is what allows it to read the
Security event channel and query other services' state.

## What it sends

`POST {server.url}/detector`

```json
{
  "agent": "SUKH-PARKING-01",
  "agentVersion": "1.0.0",
  "baiguullagiinId": "...",
  "baiguullagiinNer": "...",
  "barilgiinId": "...",
  "os": "Windows 10 Pro (build 10.0.19045)",
  "events": [
    {
      "turul": "service",
      "system": "windows",
      "modul": "DahuaParkingService",
      "aldaaniiMsg": "Үйлчилгээ 'Dahua Parking' зогссон: Stopped",
      "aldaaniiKod": "service/Stopped",
      "aldaaniiTuvshin": "Critical",
      "stackTrace": "",
      "ognoo": "2026-08-17T09:12:33+08:00",
      "fingerprint": "9f2a41c0b7d3e885",
      "davtalt": 1,
      "nemelt": { "state": "Stopped", "exitCode": 1067, "pid": 0 }
    }
  ]
}
```

`POST {server.url}/detector/heartbeat` every 60s carries agent liveness, the
current state of every watched service, and the CPU / RAM / disk snapshot — that
is what makes an agent show as **Offline** on the dashboard when a whole machine
drops off, which no error report could ever tell you.

## Server side

Three pieces ship with the agent and go into the existing projects:

**udirdlagaBack**

| File | |
|------|--|
| `models/detector.js` | one row per fingerprint per machine |
| `models/detectorAgent.js` | one row per machine, updated by the heartbeat |
| `routes/detectorRoute.js` | `POST /detector`, `POST /detector/heartbeat`, `GET /detector`, `GET /detector/statistics`, `GET /detector/agents`, `PUT /detector/:id`, `PUT /detector/olontoo`, `DELETE /detector/:id` |
| `index.js` | registers the route + a nightly cleanup cron |

Set `DETECTOR_TOKEN` in `tokhirgoo/tokhirgoo.env` to require a shared secret
from every agent; leave it unset and the endpoint stays open like `/aldaa`.
Whatever you set there goes into each agent's `[server].token`.

Cleanup runs at 03:25 daily: resolved rows older than 30 days and `Info` rows
older than 7 days are deleted, so the collection does not grow without bound.

**zevtabs**

| File | |
|------|--|
| `hooks/useDetector.js` | SWR hooks — list, statistics, agents |
| `pages/khyanalt/detector/index.js` | the **Detector** page (Машинууд / Илэрсэн асуудал / Статистик) |
| `components/Admin.js` | one entry added to `khuudasnuud` so the page shows in the menu |

The page is permission-gated by `khuudasniiNer: "detector"` like every other
page, so give the relevant staff that window in Ажилтан бүртгэл before they can
open it.

## Tuning the noise

The Event Log is loud by default. Three knobs, on the **Event Log** screen (or
in `[eventlog]`):

- `max_level = 2` — errors and criticals only, drop warnings.
- `providers = ["Service Control Manager", "Application Error"]` — whitelist.
- `ignore_event_ids` / `ignore_contains` — targeted mutes.

Start at `max_level = 3` for a week, see what floods the dashboard, then mute it.
