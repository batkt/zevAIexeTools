# Handoff — what changed while you were away

Everything below is built and tested. Nothing was installed, deployed, committed
or switched on. Three things wait on you; they are listed at the bottom.

## Agent — new in 1.2.0

| Module | What it does |
|--------|--------------|
| `heal.rs` | Restarts stopped services, and stop-then-starts wedged ones. Rolling-hour cap, then one Critical asking for a human. |
| `netqual.rs` | 0–100 network score from latency, loss, failed uploads. |
| `ports.rs` | Watches local TCP ports (37777, 5000, 554…). |
| `timesync.rs` | Clock drift vs the server; `w32tm /resync` above threshold. |
| `cleanup.rs` | Weekly disk cleanup from a hardcoded allowlist. |
| `diag.rs` | Ten read-only diagnostics runnable from the dashboard. |
| `update.rs` | Self-update from GitHub releases. |
| `tushaal.rs` | Executes commands; swaps service binaries with rollback. |

**36 unit tests**, all passing (`cargo test --lib`).

## The things most likely to bite, and what was done about them

**Cleanup deletes files unattended as LocalSystem.** The roots are hardcoded, not
configurable, so a config typo cannot point it at `C:\`. Every candidate is
re-checked against its root *after* canonicalising, and reparse points are never
followed or removed — `C:\Windows\Temp` can hold a junction to anywhere on the
disk, and without that check walking it would delete the target. Files only,
never directories. Nothing newer than 7 days. Browser caches deliberately
excluded: clearing them signs the kiosk browser out of whatever it was logged
into. There is a test asserting the allowlist can never contain a drive root.

**Binary swaps replace a running exe.** The path comes from the SCM's own
`query_config()`, not a guess — dahuarust is installed from wherever `install`
ran (its own `target\release`), so replacing an assumed Program Files path would
silently do nothing and look like a failed update. SHA-256 is verified before
anything is touched. The old exe is kept and restored if the new one fails to
start *or* fails its health probe. Only the exe moves; `config.toml`, the Dahua
DLLs and `service.log` are left alone.

**Packet loss on localised Windows.** `ping.exe` translates "0% loss", so
scraping its output breaks on exactly the machines this runs on. Loss is counted
from exit codes instead. Latency is measured in-process by TCP connect, because
spawning `ping.exe` costs 5–15 ms — the same order as the number being measured.
Blocked ICMP reports loss as *unknown*, not 100%.

**Version comparison.** Numeric, not textual: `1.10.0` really is newer than
`1.9.0`, which a string compare gets backwards and would have silently stopped
every update after 1.9. A tag like `nightly` never counts as newer.

**Remote control is unauthenticated by default.** The pre-existing token check
fails *open* when `DETECTOR_TOKEN` is unset — fine for ingest, but it would have
meant unauthenticated code execution as LocalSystem for commands. The new
endpoints use a separate check that fails *closed*: no token, no remote control
(503). Operator endpoints require the ajiltan JWT.

## Backend — `udirdlagaBack`

- `models/detectorTushaal.js` — command queue, delivered on the heartbeat
  response (these machines are behind NAT; there is no port to open).
- `models/detectorHuvilbar.js` — uploaded binaries with server-computed SHA-256.
- `detectorAgent` gained `sergeelt`, `suljee`, `portuud`, `tsag`.
- Route order verified programmatically — `GET /detector/tushaal` would otherwise
  be silently swallowed by `GET /detector/:id`.

## zevtabs

- **Удирдлага tab** — per-machine restart/start/stop/test/update, ten diagnostic
  buttons, cleanup (plus a dry-run "see what it would delete"), binary upload,
  version activation, command history.
- **Re-login on token expiry.** The real bug: the `khereglegch` cookie lasts 30
  days but the JWT inside expires in 12 hours, so people were thrown to `/`
  mid-work. Now one modal (deduped — every in-flight request fails at once) and
  you stay on the same page.
- **Stale-build prompt** using Next's own build id, with a real cache-clearing
  hard reload.

`next build` clean.

## dahua-service

Added `--version` and the version in its startup log line. It previously had no
way to report which build was running, so a rollout could never be verified.

## Waiting on you

1. **GitHub repo for self-update.** `[update].github_repo` is empty, so
   self-update is entirely off. Set `owner/name` (plus `github_token` if the repo
   is private) and publish the installer as a release asset named `*Setup*.exe`.
2. **`DETECTOR_TOKEN`.** Remote control returns 503 until it is set. **Order
   matters:** put the token in each agent's Token field *first*, then add
   `DETECTOR_TOKEN` to `tokhirgoo.env`. The other way round will 401 your three
   live agents' ingest and they will stop reporting.
3. **Reinstall on all three machines.** DESKTOP-C3LTGKT, DESKTOP-DDRAV77 and
   MEMAR are all on the old build. Also worth noting: DESKTOP-DDRAV77 and MEMAR
   have `services = []`, so self-healing has nothing to act on there until a
   service is configured.

## Not done

The dashboard shows the new heartbeat fields (`suljee`, `portuud`, `tsag`) only
as raw data — the network-score bar and port grid mockup from your list are not
drawn yet. The data is arriving; it is presentation that is missing.
