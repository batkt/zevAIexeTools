/* Preview-only backend stub.
 *
 * Inside the app this file does nothing: Tauri injects `window.__TAURI__`
 * before any page script runs, so the guard below is already false. It exists
 * so index.html can be opened directly in a browser to work on the CSS
 * without rebuilding the Rust binaries — every command answers with plausible
 * fixture data instead of touching the machine.
 *
 * Set `?fresh=1` on the URL to preview the first-run wizard.
 */
if (!window.__TAURI__) {
  const fresh = new URLSearchParams(location.search).has("fresh");

  const fixture = {
    server: {
      url: "https://admin.zevtabs.mn/api",
      token: "",
      timeout_secs: 20,
      retry_count: 3,
      batch_size: 50,
      send_interval_secs: 30,
      queue_max: 5000,
    },
    agent: {
      name: "SUKH-PARKING-01",
      baiguullagiinId: "64f1c2a9b7d3e8850a1b2c3d",
      baiguullagiinNer: "Сүхбаатар зогсоол",
      barilgiinId: "64f1c2a9b7d3e8850a1b2c40",
      heartbeat_interval_secs: 60,
    },
    eventlog: {
      enabled: true,
      poll_interval_secs: 60,
      channels: ["System", "Application"],
      max_level: 3,
      providers: [],
      ignore_event_ids: [10016],
      ignore_contains: [],
      max_per_poll: 200,
    },
    services: [
      {
        name: "DahuaParkingService", label: "Dahua Parking (dahuarust)", must_run: true,
        auto_restart: true, max_restarts_per_hour: 3, restart_grace_secs: 45,
      },
    ],
    logs: {
      enabled: true,
      poll_interval_secs: 15,
      files: [{
        path: "D:\\zevtabs\\dahuarust\\dahuaExeRust\\target\\release\\service.log",
        glob: "", system: "dahuarust", modul: "dahua-service",
        pattern: "", ignore_pattern: "", context_lines: 8,
      }],
    },
    health: {
      enabled: true,
      poll_interval_secs: 60,
      cpu_threshold: 90,
      mem_threshold: 90,
      disk_threshold: 90,
      http: [{
        name: "dahua api", url: "http://127.0.0.1:5000/api/health",
        expect: "OK", timeout_secs: 5, system: "dahuarust",
        service: "DahuaParkingService", restart_after_failures: 3,
      }],
    },
    netqual: {
      enabled: true,
      poll_interval_secs: 300,
      ping_host: "",
      ping_count: 10,
      score_threshold: 70,
    },
    ports: [
      { port: 37777, label: "Dahua SDK", host: "127.0.0.1", must_listen: true },
      { port: 5000, label: "Parking service", host: "127.0.0.1", must_listen: true },
    ],
    update: {
      mode: "ask",
      check_interval_hours: 6,
      github_repo: "",
      github_token: "",
      window: "",
    },
    timesync: {
      enabled: true,
      poll_interval_secs: 900,
      max_drift_secs: 30,
      auto_resync: true,
    },
    cleanup: {
      enabled: true,
      weekday: 0,
      hour: 3,
      min_age_days: 7,
      disk_trigger_percent: 92,
      log_max_mb: 32,
    },
  };

  let state = { installed: !fresh, state: fresh ? null : "Running", pid: fresh ? null : 8412 };

  const handlers = {
    agent_info: () => ({
      version: "1.0.0",
      hostname: "SUKH-PARKING-01",
      os: "Windows 11 Pro (build 26200)",
      configPath: "C:\\Program Files\\Zev Detector\\config.toml",
      logPath: "C:\\Program Files\\Zev Detector\\zev-detector.log",
      configExists: !fresh,
    }),
    load_config: () => structuredClone(fixture),
    save_config: () => null,
    service_status: () => state,
    service_action: ({ action }) => {
      const said = {
        install: [{ installed: true, state: "Stopped", pid: null }, "Service installed."],
        uninstall: [{ installed: false, state: null, pid: null }, "Service removed."],
        start: [{ installed: true, state: "Running", pid: 8412 }, "Service started."],
        stop: [{ installed: true, state: "Stopped", pid: null }, "Service stopped."],
      }[action];
      state = said[0];
      return said[1];
    },
    run_test: () =>
      [
        "Agent   : SUKH-PARKING-01",
        "Server  : https://admin.zevtabs.mn/api/detector",
        "OS      : Windows 11 Pro (build 26200)",
        "",
        "Event log  : 2 event(s)",
        "Services   : 1 event(s)",
        '             {"label":"Dahua Parking","name":"DahuaParkingService","state":"Running"}',
        "Log files  : 0 line(s)",
        "Health     : 1 event(s)",
        "",
        "─── collected ───────────────────────────────",
        "[Warning] windows / Application :: ECR not connected, attempting to reconnect...",
        "[Critical] windows / disk :: Диск C:\\ дүүрч байна: 98% ашиглагдсан, 6.8 GB чөлөөтэй",
      ].join("\n"),
    pick_file: () => "D:\\zevtabs\\worker\\logs\\worker.log",
    read_log: () =>
      [
        "[2026-08-18 09:04:11] [INFO] === Zev Detector 1.0.0 starting ===",
        "[2026-08-18 09:04:11] [INFO] Agent: SUKH-PARKING-01 → https://admin.zevtabs.mn/api",
        "[2026-08-18 09:04:11] [INFO] Watching: 2 event log channel(s), 1 service(s), 1 log file target(s), 1 http probe(s)",
        "[2026-08-18 09:04:26] [INFO] Event log: 2 new event(s)",
        "[2026-08-18 09:04:41] [WARN] Health: disk C:\\ at 98%",
        "[2026-08-18 09:05:11] [INFO] Sent 3 event(s)",
      ].join("\n"),
    list_installed_services: () => [
      { name: "DahuaParkingService", displayName: "zevDahuaRust", status: "Running", isParkingRelated: true },
      { name: "ParkingService", displayName: "zevzogsoolrust", status: "Running", isParkingRelated: true },
      { name: "zevdahua", displayName: "zevdahua", status: "Running", isParkingRelated: true },
      { name: "zevstream", displayName: "zevstream", status: "Running", isParkingRelated: true },
      { name: "ZevDetector", displayName: "Zev Detector", status: "Running", isParkingRelated: true },
      { name: "Spooler", displayName: "Print Spooler", status: "Running", isParkingRelated: false },
      { name: "W32Time", displayName: "Windows Time", status: "Stopped", isParkingRelated: false },
    ],
  };

  window.__TAURI__ = {
    core: {
      invoke: (cmd, args = {}) =>
        new Promise((resolve, reject) =>
          setTimeout(() => {
            const fn = handlers[cmd];
            fn ? resolve(fn(args)) : reject(`mock: unknown command ${cmd}`);
          }, 120)
        ),
    },
  };
  console.info("[zev] preview mode — using mock backend");
}
