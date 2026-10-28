/* Zev Detector — desktop app frontend.
 *
 * State is one plain `cfg` object mirroring config.toml. Scalar inputs bind to
 * it declaratively through `data-bind="section.key"`, so adding a setting to
 * the Rust config struct and to the HTML is enough — there is no per-field
 * wiring to keep in sync here. Repeating sections (services, log files, HTTP
 * probes) are rendered from templates instead, since they are arrays.
 */

const invoke = window.__TAURI__.core.invoke;

let cfg = null;      // the config as last loaded or edited
let saved = "";      // JSON of the config as it exists on disk
let info = null;     // agent_info() result
let statusTimer = null;

// ─── tiny helpers ────────────────────────────────────────────────────────
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));

function get(obj, path) {
  return path.split(".").reduce((o, k) => (o == null ? undefined : o[k]), obj);
}
function set(obj, path, value) {
  const keys = path.split(".");
  const last = keys.pop();
  const target = keys.reduce((o, k) => (o[k] ??= {}), obj);
  target[last] = value;
}

function toast(message, kind = "") {
  const el = document.createElement("div");
  el.className = `toast ${kind}`;
  el.textContent = message;
  $("#toasts").appendChild(el);
  setTimeout(() => {
    el.classList.add("leave");
    setTimeout(() => el.remove(), 200);
  }, 4200);
}

function busy(on, text = "Ажиллаж байна…") {
  $("#busy-text").textContent = text;
  $("#busy").hidden = !on;
}

// ─── binding ─────────────────────────────────────────────────────────────
/** Writes cfg values into every [data-bind] control. */
function renderBindings(root = document) {
  for (const el of $$("[data-bind]", root)) {
    const value = get(cfg, el.dataset.bind);
    if (el.type === "checkbox") {
      el.checked = !!value;
    } else if (el.hasAttribute("data-list")) {
      el.value = Array.isArray(value) ? value.join("\n") : "";
    } else {
      el.value = value ?? "";
    }
    syncOutput(el);
  }
  for (const el of $$("[data-text]", root)) {
    const key = el.dataset.text;
    const value = get(cfg, key) ?? get(info, key);
    if (value != null && value !== "") el.textContent = value;
  }
}

/** Mirrors a range input's value into its <b data-out> readout. */
function syncOutput(el) {
  if (el.type !== "range") return;
  const out = $(`[data-out="${el.dataset.bind}"]`);
  if (out) out.textContent = Math.round(Number(el.value));
}

function readControl(el) {
  if (el.type === "checkbox") return el.checked;
  if (el.hasAttribute("data-list")) {
    const lines = el.value.split("\n").map((s) => s.trim()).filter(Boolean);
    return el.hasAttribute("data-numeric")
      ? lines.map(Number).filter((n) => Number.isFinite(n))
      : lines;
  }
  if (el.type === "number" || el.type === "range" || el.tagName === "SELECT") {
    const n = Number(el.value);
    return Number.isFinite(n) ? n : 0;
  }
  return el.value;
}

/** One listener on the document covers every bound control, including the
 *  ones templates add later. */
document.addEventListener("input", (e) => {
  const el = e.target;
  if (!cfg || !el.dataset || !el.dataset.bind) return;
  set(cfg, el.dataset.bind, readControl(el));
  syncOutput(el);
  markDirty();
});

function isDirty() {
  return cfg && JSON.stringify(cfg) !== saved;
}
function markDirty() {
  $("#save-btn").disabled = !isDirty();
  renderWatchList();
  $("#count-services").textContent = cfg.services.length;
  $("#count-logs").textContent = cfg.logs.files.length;
}

// ─── repeating sections ──────────────────────────────────────────────────
/** Renders `items` into `container` from `tplId`, wiring each [data-k] input
 *  back to its own array element. */
function renderList(container, tplId, items, onChange) {
  container.textContent = "";
  items.forEach((item, index) => {
    const node = $("#" + tplId).content.firstElementChild.cloneNode(true);

    for (const input of $$("[data-k]", node)) {
      const key = input.dataset.k;
      if (input.type === "checkbox") input.checked = !!item[key];
      else input.value = item[key] ?? "";

      input.addEventListener("input", () => {
        item[key] =
          input.type === "checkbox" ? input.checked
          : input.type === "number" ? Number(input.value) || 0
          : input.value;
        markDirty();
      });
    }

    $(".remove", node).addEventListener("click", () => {
      items.splice(index, 1);
      onChange();
      markDirty();
    });

    const browse = $(".browse", node);
    if (browse) {
      browse.addEventListener("click", async () => {
        const picked = await invoke("pick_file");
        if (picked) {
          item.path = picked;
          $('[data-k="path"]', node).value = picked;
          markDirty();
        }
      });
    }

    container.appendChild(node);
  });
}

let installedServices = [];

async function loadInstalledServices() {
  try {
    installedServices = await invoke("list_installed_services");
  } catch (err) {
    console.warn("Could not list installed services:", err);
    installedServices = [];
  }
}

function renderServices() {
  const container = $("#services-list");
  container.textContent = "";

  cfg.services.forEach((item, index) => {
    const node = $("#tpl-service").content.firstElementChild.cloneNode(true);

    const titleEl = $(".service-name-title", node);
    if (titleEl) {
      titleEl.textContent = item.label || item.name || "Шинэ үйлчилгээ";
    }

    const statusPill = $(".service-status-pill", node);
    function updateStatusPill(st) {
      if (!statusPill) return;
      if (!st) {
        const found = installedServices.find((s) => s.name.toLowerCase() === (item.name || "").toLowerCase());
        st = found ? found.status : (item.name ? "Not Installed" : "");
      }
      if (!st) {
        statusPill.hidden = true;
      } else {
        statusPill.hidden = false;
        const isRunning = st === "Running";
        statusPill.textContent = isRunning ? "Running (Ажиллаж байна)" : st;
        statusPill.className = "service-status-pill " + (isRunning ? "running" : "stopped");
      }
    }
    updateStatusPill();

    const selectEl = $(".service-select", node);
    if (selectEl) {
      const options = [
        `<option value="">-- Компьютер дээрх үйлчилгээнүүдээс сонгох (${installedServices.length}) --</option>`
      ];
      for (const s of installedServices) {
        const selected = (s.name.toLowerCase() === (item.name || "").toLowerCase()) ? "selected" : "";
        const icon = s.isParkingRelated ? "⭐ " : "";
        options.push(
          `<option value="${escapeHtml(s.name)}" data-label="${escapeHtml(s.displayName)}" data-status="${escapeHtml(s.status)}" ${selected}>` +
          `${icon}${escapeHtml(s.displayName)} (${escapeHtml(s.name)}) [${escapeHtml(s.status)}]</option>`
        );
      }
      selectEl.innerHTML = options.join("");

      selectEl.addEventListener("change", () => {
        const selectedOpt = selectEl.selectedOptions[0];
        if (selectedOpt && selectedOpt.value) {
          item.name = selectedOpt.value;
          item.label = selectedOpt.dataset.label || selectedOpt.value;
          $('[data-k="name"]', node).value = item.name;
          $('[data-k="label"]', node).value = item.label;
          if (titleEl) titleEl.textContent = item.label;
          updateStatusPill(selectedOpt.dataset.status);
          markDirty();
        }
      });
    }

    for (const input of $$("[data-k]", node)) {
      const key = input.dataset.k;
      if (input.type === "checkbox") input.checked = !!item[key];
      else input.value = item[key] ?? "";

      input.addEventListener("input", () => {
        item[key] =
          input.type === "checkbox" ? input.checked
          : input.type === "number" ? Number(input.value) || 0
          : input.value;
        if (key === "name" || key === "label") {
          if (titleEl) titleEl.textContent = item.label || item.name || "Шинэ үйлчилгээ";
          updateStatusPill();
        }
        markDirty();
      });
    }

    $(".remove", node).addEventListener("click", () => {
      cfg.services.splice(index, 1);
      renderServices();
      markDirty();
    });

    container.appendChild(node);
  });
}



const renderLogs = () =>
  renderList($("#logs-list"), "tpl-log", cfg.logs.files, renderLogs);
const renderProbes = () =>
  renderList($("#probes-list"), "tpl-probe", cfg.health.http, renderProbes);
const renderPorts = () => renderList($("#ports-list"), "tpl-port", cfg.ports, renderPorts);

function renderWatchList() {
  const rows = [
    ["Windows Event Log", cfg.eventlog.enabled, `${cfg.eventlog.channels.length} суваг`],
    ["Windows үйлчилгээ", cfg.services.length > 0, `${cfg.services.length} үйлчилгээ`],
    ["Лог файл", cfg.logs.enabled && cfg.logs.files.length > 0, `${cfg.logs.files.length} файл`],
    ["Эрүүл мэнд", cfg.health.enabled, `${cfg.health.http.length} HTTP шалгалт`],
  ];
  $("#watch-list").innerHTML = "";
  for (const [label, on, detail] of rows) {
    const li = document.createElement("li");
    const name = document.createElement("span");
    name.textContent = label;
    const tag = document.createElement("span");
    tag.className = on ? "tag" : "tag off";
    tag.textContent = on ? detail : "унтраалттай";
    li.append(name, tag);
    $("#watch-list").appendChild(li);
  }
}

// ─── service status ──────────────────────────────────────────────────────
async function refreshStatus() {
  let s;
  try {
    s = await invoke("service_status");
  } catch {
    return;
  }
  const pill = $("#status-pill");
  const running = s.installed && s.state === "Running";

  pill.className = "status-pill " + (!s.installed ? "missing" : running ? "running" : "stopped");
  $("#status-text").textContent = !s.installed
    ? "Суулгаагүй"
    : running
    ? "Ажиллаж байна"
    : s.state ?? "Тодорхойгүй";

  $("#hero-state").textContent = !s.installed
    ? "Үйлчилгээ суулгаагүй"
    : running
    ? "Ажиллаж байна"
    : `Зогссон — ${s.state ?? "?"}`;
  $("#hero-sub").textContent = s.pid ? `Windows үйлчилгээ · PID ${s.pid}` : "Windows үйлчилгээ";

  $('[data-svc="install"]').disabled = s.installed;
  $('[data-svc="uninstall"]').disabled = !s.installed;
  $('[data-svc="start"]').disabled = !s.installed || running;
  $('[data-svc="stop"]').disabled = !s.installed || !running;

  await checkActiveAlerts();
}

let currentActiveAlerts = [];

async function checkActiveAlerts() {
  try {
    currentActiveAlerts = await invoke("get_active_alerts");
    renderActiveAlerts();
  } catch (err) {
    // ignore
  }
}

function renderActiveAlerts() {
  const banner = $("#local-alert-banner");
  if (!banner) return;

  if (!currentActiveAlerts || currentActiveAlerts.length === 0) {
    banner.hidden = true;
    return;
  }

  banner.hidden = false;
  const isCritical = currentActiveAlerts.some((a) => a.severity === "Critical");
  banner.classList.toggle("critical", isCritical);

  const top = currentActiveAlerts[0];
  $("#alert-icon").textContent = isCritical ? "🚨" : "💡";
  $("#alert-title").textContent = currentActiveAlerts.length > 1
    ? `${top.title} (+${currentActiveAlerts.length - 1} анхааруулга)`
    : top.title;
  $("#alert-msg").textContent = top.message;
}

function showAlertModal() {
  const modal = $("#alert-modal");
  const body = $("#alert-modal-body");
  if (!modal || !body) return;

  body.innerHTML = "";
  if (!currentActiveAlerts || currentActiveAlerts.length === 0) {
    body.innerHTML = `<div style="text-align:center;color:var(--text-2);padding:20px;">Идэвхтэй асуудал илрээгүй. Бүх систем хэвийн байна.</div>`;
  } else {
    for (const a of currentActiveAlerts) {
      const card = document.createElement("div");
      const sevClass = (a.severity || "").toLowerCase();
      card.className = `alert-item-card ${sevClass}`;
      card.innerHTML = `
        <div class="alert-item-title">${a.title}</div>
        <div class="alert-item-msg">${a.message}</div>
        <div class="alert-item-advice">💡 <b>Зөвлөмж:</b> ${a.advice}</div>
      `;
      body.appendChild(card);
    }
  }
  modal.hidden = false;
}

function closeAlertModal() {
  const modal = $("#alert-modal");
  if (modal) modal.hidden = true;
}

async function serviceAction(action) {
  const labels = {
    install: "Үйлчилгээ суулгаж байна…",
    uninstall: "Үйлчилгээг устгаж байна…",
    start: "Асааж байна…",
    stop: "Унтрааж байна…",
  };
  busy(true, labels[action]);
  try {
    toast(await invoke("service_action", { action }), "ok");
  } catch (e) {
    toast(String(e), "err");
  } finally {
    busy(false);
    await refreshStatus();
  }
}

// ─── dahua parking service monitor ────────────────────────────────────────
let latestDahuaSnapshot = null;
let dahuaPollTimer = null;
let isDahuaRefreshing = false;

function escapeHtml(str) {
  if (!str) return "";
  return String(str)
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#039;");
}

async function refreshDahuaStatus() {
  if (isDahuaRefreshing) return;
  isDahuaRefreshing = true;
  try {
    const s = await invoke("dahua_status");
    latestDahuaSnapshot = s;
    renderDahuaUI(s);
  } catch (err) {
    console.error("Dahua status query error:", err);
  } finally {
    isDahuaRefreshing = false;
  }
}

function renderDahuaUI(s) {
  if (!s) return;

  const isHik = s.provider === "hikvision" || (s.displayName && s.displayName.toLowerCase().includes("hik")) || (s.serviceName && s.serviceName.toLowerCase().includes("hik"));

  // 0. Dynamic titles
  const navTitle = $("#nav-dahua-title");
  if (navTitle) navTitle.textContent = isHik ? "Hikvision Parking" : "Dahua Parking";

  const ovMainTitle = $("#ov-dahua-main-title");
  if (ovMainTitle) ovMainTitle.textContent = isHik ? "Hikvision Parking Service (hikRustExe)" : "Dahua Parking Service (dahuarust)";

  const ovSub = $("#ov-dahua-sub");
  if (ovSub) ovSub.textContent = isHik ? "Hikvision ALPR камер, хаалт болон орон нутгийн API үйлчилгээ" : "Dahua ALPR камер, хаалт болон API үйлчилгээ";

  // 1. Sidebar pill
  const sidePill = $("#dahua-sidebar-pill");
  if (sidePill) {
    sidePill.textContent = s.isActive ? "Идэвхтэй" : "Зогссон";
    sidePill.className = "pill " + (s.isActive ? "running" : "stopped");
  }

  // 2. Overview card
  const ovBadge = $("#ov-dahua-state-badge");
  if (ovBadge) {
    ovBadge.textContent = s.isActive ? `Идэвхтэй (${s.state})` : `Зогссон (${s.state})`;
    ovBadge.className = "dahua-pill " + (s.isActive ? "" : "stopped");
  }
  const ovPort = $("#ov-dahua-port5000");
  if (ovPort) {
    ovPort.textContent = `Port 5000: ${s.apiPort5000Open ? "Нээлттэй" : "Хаалттай"}`;
    ovPort.style.color = s.apiPort5000Open ? "#10b981" : "#ef4444";
  }
  const ovLog = $("#ov-dahua-log-text");
  if (ovLog) {
    if (s.recentLogs && s.recentLogs.length > 0) {
      const lastRec = s.recentLogs[s.recentLogs.length - 1];
      ovLog.textContent = `[${lastRec.timestamp}] [${lastRec.level}] ${lastRec.message}`;
    } else {
      ovLog.textContent = "Лог бичлэг олдсонгүй";
    }
  }

  // 3. Dahua/Hikvision dedicated page
  const titleEl = $("#dh-svc-title");
  if (titleEl) titleEl.textContent = `${s.serviceName} (${s.displayName})`;

  const dhSub = $("#dh-svc-sub");
  if (dhSub) dhSub.textContent = isHik ? "Hikvision ALPR камер, хаалт ба API серверийн шууд хяналт" : "Dahua ALPR камер, хаалт ба API серверийн шууд хяналт";

  const statePill = $("#dh-state-pill");
  if (statePill) {
    statePill.className = "dahua-state-pill " + (s.isActive ? "" : "stopped");
    $("#dh-state-text").textContent = s.state || (s.isActive ? "Ажиллаж байна" : "Зогссон");
  }

  const pidEl = $("#dh-stat-pid");
  if (pidEl) pidEl.textContent = s.pid ? `${s.state} · PID ${s.pid}` : s.state;

  const p5000El = $("#dh-stat-port5000");
  if (p5000El) {
    p5000El.textContent = s.apiPort5000Open ? "Нээлттэй (Active)" : "Хаалттай (Closed)";
    p5000El.style.color = s.apiPort5000Open ? "#10b981" : "#ef4444";
  }

  const p2Label = $("#dh-port2-label");
  if (p2Label) p2Label.textContent = isHik ? "Камер Порт 80" : "NetSDK Порт 37777";

  const p37777El = $("#dh-stat-port37777");
  if (p37777El) {
    if (isHik) {
      p37777El.textContent = s.cameraPort80Open ? "Нээлттэй (Online)" : (s.isActive ? "Холбогдсон" : "Хаалттай (Closed)");
      p37777El.style.color = (s.cameraPort80Open || s.isActive) ? "#10b981" : "var(--text-2)";
    } else {
      p37777El.textContent = s.netsdkPort37777Open ? "Нээлттэй (Active)" : "Хаалттай (Closed)";
      p37777El.style.color = s.netsdkPort37777Open ? "#10b981" : "var(--text-2)";
    }
  }

  const updatedEl = $("#dh-stat-updated");
  if (updatedEl) updatedEl.textContent = s.updatedAt || "—";

  const logPathEl = $("#dh-log-path");
  if (logPathEl) logPathEl.textContent = s.logPath + (s.logExists ? " (Байгаа)" : " (Файл үүсээгүй)");

  // Render 10 records FIFO table
  const rowsEl = $("#dh-log-rows");
  if (rowsEl) {
    if (!s.recentLogs || s.recentLogs.length === 0) {
      rowsEl.innerHTML = `<div class="terminal-empty">Хадгалагдсан бичлэг алга байна (${s.logPath})</div>`;
    } else {
      rowsEl.innerHTML = s.recentLogs.map((r, idx) => {
        const lvl = (r.level || "INFO").toLowerCase();
        return `
          <div class="dahua-terminal-row">
            <span class="terminal-col-num">${idx + 1}</span>
            <span class="terminal-col-time">${escapeHtml(r.timestamp)}</span>
            <div><span class="terminal-level-badge ${lvl}">${escapeHtml(r.level)}</span></div>
            <span class="terminal-col-msg">${escapeHtml(r.message)}</span>
          </div>
        `;
      }).join("");
    }
  }

  // Button disabled states
  const btnStart = $("#dh-btn-start");
  if (btnStart) btnStart.disabled = s.isActive;
  const btnStop = $("#dh-btn-stop");
  if (btnStop) btnStop.disabled = !s.isActive;
}

async function dahuaAction(action) {
  const labels = {
    start: "Dahua үйлчилгээг асааж байна…",
    stop: "Dahua үйлчилгээг зогсоож байна…",
    restart: "Dahua үйлчилгээг дахин ачаалж байна…",
  };
  busy(true, labels[action] || "Ажиллаж байна…");
  try {
    const res = await invoke("dahua_action", { action });
    toast(res, "ok");
  } catch (e) {
    toast(String(e), "err");
  } finally {
    busy(false);
    await refreshDahuaStatus();
  }
}

// ─── saving ──────────────────────────────────────────────────────────────
async function save() {
  try {
    await invoke("save_config", { cfg });
    saved = JSON.stringify(cfg);
    markDirty();
    toast("Тохиргоо хадгалагдлаа.", "ok");
    return true;
  } catch (e) {
    toast(String(e), "err");
    return false;
  }
}

// ─── navigation ──────────────────────────────────────────────────────────
const PAGE_META = {
  overview: ["Тойм", "Үйлчилгээний төлөв ба удирдлага"],
  server:   ["Холболт", "udirdlagaBack хаяг ба таних тэмдэг"],
  eventlog: ["Event Log", "Windows-ийн үйл явдлын бүртгэл"],
  services: ["Үйлчилгээ", "Компьютер дээрх Windows үйлчилгээнүүд"],
  dahuarust: ["Dahua Parking", "Dahua ALPR камер, хаалт болон API үйлчилгээний шууд хяналт"],
  logfiles: ["Лог файл", "Програмын лог мөрүүд"],
  health:   ["Эрүүл мэнд", "CPU, RAM, диск, порт ба сүлжээ"],
  maintenance: ["Цаг & цэвэрлэлт", "Цагийн зөрүү ба хуваарьт цэвэрлэлт"],
  update:   ["Шинэчлэлт", "GitHub release-ээс өөрөө шинэчлэх"],
  logview:  ["Бүртгэл", "zev-detector.log"],
};

function goto(page) {
  $$(".nav-item").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
  $$(".page").forEach((s) => s.classList.toggle("active", s.dataset.page === page));
  const [title, sub] = PAGE_META[page] || [page, ""];
  $("#page-title").textContent = title;
  $("#page-sub").textContent = sub;
  if (page === "logview") loadLog();
  if (page === "dahuarust" || page === "overview") refreshDahuaStatus();
  if (page === "update") initUpdatePage();
}

async function loadLog() {
  try {
    $("#log-out").textContent = await invoke("read_log", { lines: 400 });
  } catch (e) {
    $("#log-out").textContent = String(e);
  }
}

// ─── theme ───────────────────────────────────────────────────────────────
function applyTheme(theme) {
  document.documentElement.dataset.theme = theme;
  localStorage.setItem("theme", theme);
}

function renderAll() {
  renderBindings();
  renderServices();
  renderLogs();
  renderProbes();
  renderPorts();
  markDirty();
  refreshStatus();
}

// ─── boot ────────────────────────────────────────────────────────────────
async function boot() {
  applyTheme(
    localStorage.getItem("theme") ||
      (matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light")
  );

  info = await invoke("agent_info");
  cfg = await invoke("load_config");
  saved = JSON.stringify(cfg);

  await loadInstalledServices();

  if (!cfg.server.url) cfg.server.url = "https://admin.zevtabs.mn/api";

  renderAll();

  await refreshDahuaStatus();
  statusTimer = setInterval(refreshStatus, 6000);
  dahuaPollTimer = setInterval(() => {
    const auto = $("#dh-auto-refresh");
    if (!auto || auto.checked) {
      refreshDahuaStatus();
    }
  }, 4000);
}

// ─── events ──────────────────────────────────────────────────────────────
$$(".nav-item").forEach((b) => b.addEventListener("click", () => goto(b.dataset.page)));
$$("[data-svc]").forEach((b) => b.addEventListener("click", () => serviceAction(b.dataset.svc)));

$("#dh-btn-start")?.addEventListener("click", () => dahuaAction("start"));
$("#dh-btn-stop")?.addEventListener("click", () => dahuaAction("stop"));
$("#dh-btn-restart")?.addEventListener("click", () => dahuaAction("restart"));
$("#dh-btn-refresh")?.addEventListener("click", refreshDahuaStatus);

$("#ov-dahua-open-btn")?.addEventListener("click", (e) => {
  e.stopPropagation();
  goto("dahuarust");
});
$("#ov-dahua-card")?.addEventListener("click", () => goto("dahuarust"));

$("#dh-copy-logs")?.addEventListener("click", () => {
  if (!latestDahuaSnapshot || !latestDahuaSnapshot.recentLogs || latestDahuaSnapshot.recentLogs.length === 0) {
    toast("Хуулах лог бичлэг алга байна.", "err");
    return;
  }
  const lines = latestDahuaSnapshot.recentLogs
    .map((r, i) => `[${i + 1}] [${r.timestamp}] [${r.level}] ${r.message}`)
    .join("\n");
  navigator.clipboard.writeText(lines).then(() => {
    toast("Сүүлийн 10 бичлэг санах ойд хуулагдлаа.", "ok");
  }).catch(() => {
    toast("Хуулахад алдаа гарлаа.", "err");
  });
});

$("#refresh-installed-services")?.addEventListener("click", async () => {
  busy(true, "Үйлчилгээнүүдийг шинэчлэн шалгаж байна…");
  await loadInstalledServices();
  busy(false);
  renderServices();
  toast(`${installedServices.length} үйлчилгээ олдлоо.`, "ok");
});

$("#save-btn").addEventListener("click", save);
$("#add-service").addEventListener("click", () => {
  cfg.services.push({
    name: "", label: "", must_run: true,
    auto_restart: true, max_restarts_per_hour: 3, restart_grace_secs: 45,
  });
  renderServices();
  markDirty();
});
$("#add-log").addEventListener("click", () => {
  cfg.logs.files.push({ path: "", glob: "", system: "", modul: "", pattern: "", ignore_pattern: "", context_lines: 8 });
  renderLogs();
  markDirty();
});
$("#add-probe").addEventListener("click", () => {
  cfg.health.http.push({
    name: "", url: "", expect: "", timeout_secs: 5, system: "",
    service: "", restart_after_failures: 3,
  });
  renderProbes();
  markDirty();
});
$("#add-port").addEventListener("click", () => {
  cfg.ports.push({ port: 0, label: "", host: "127.0.0.1", must_listen: true });
  renderPorts();
  markDirty();
});

$("#test-btn").addEventListener("click", async () => {
  if (isDirty() && !(await save())) return;
  busy(true, "Цуглуулж байна…");
  try {
    const out = $("#test-out");
    out.textContent = await invoke("run_test");
    out.hidden = false;
  } catch (e) {
    toast(String(e), "err");
  } finally {
    busy(false);
  }
});

$("#refresh-log").addEventListener("click", loadLog);
$("#theme-toggle").addEventListener("click", () =>
  applyTheme(document.documentElement.dataset.theme === "dark" ? "light" : "dark")
);



$("#alert-details-btn")?.addEventListener("click", showAlertModal);
$("#local-alert-banner")?.addEventListener("click", (e) => {
  if (e.target.id !== "alert-details-btn") showAlertModal();
});
$("#alert-modal-close")?.addEventListener("click", closeAlertModal);
$("#alert-modal-ok")?.addEventListener("click", closeAlertModal);

window.addEventListener("keydown", (e) => {
  if (e.key === "Escape") closeAlertModal();
  if ((e.ctrlKey || e.metaKey) && e.key === "s") {
    e.preventDefault();
    if (isDirty()) save();
  }
});

function initUpdatePage() {
  const curVer = info?.version || "1.7.0";
  const el = $("#update-current-ver");
  if (el) el.textContent = `v${curVer}`;
}

$("#btn-check-update")?.addEventListener("click", async () => {
  const btn = $("#btn-check-update");
  const applyBtn = $("#btn-apply-update");
  const statusBox = $("#update-status-box");
  const latestVerEl = $("#update-latest-ver");

  btn.disabled = true;
  statusBox.textContent = "GitHub release шалгаж байна (batkt/zevAIexeTools)...";
  statusBox.style.color = "var(--fg)";

  try {
    const res = await invoke("check_update");
    if (latestVerEl) latestVerEl.textContent = `v${res.latestVersion}`;

    if (res.hasUpdate) {
      statusBox.innerHTML = `Шинэ хувилбар олдлоо: <b>v${res.latestVersion}</b> (${res.assetName})<br/><small style="color:var(--muted)">Нийтэлсэн: ${res.publishedAt}</small>`;
      statusBox.style.color = "#16a34a";
      if (applyBtn) applyBtn.style.display = "inline-flex";
      toast(`Шинэ хувилбар v${res.latestVersion} олдлоо!`);
    } else {
      statusBox.textContent = `Таны ашиглаж буй v${res.currentVersion} хувилбар хамгийн сүүлийнх байна.`;
      statusBox.style.color = "var(--fg)";
      if (applyBtn) applyBtn.style.display = "none";
      toast("Хамгийн сүүлийн хувилбар суусан байна.");
    }
  } catch (e) {
    statusBox.textContent = `Шалгахад алдаа гарлаа: ${e}`;
    statusBox.style.color = "#ef4444";
    toast(String(e), "err");
  } finally {
    btn.disabled = false;
  }
});

$("#btn-apply-update")?.addEventListener("click", async () => {
  const btn = $("#btn-apply-update");
  const checkBtn = $("#btn-check-update");
  const statusBox = $("#update-status-box");

  if (btn) btn.disabled = true;
  if (checkBtn) checkBtn.disabled = true;
  if (statusBox) statusBox.textContent = "Шинэ хувилбарыг татан суулгаж байна. Үйлчилгээ түр зогсож дахин асах болно...";
  busy(true, "Шинэ хувилбарыг татан суулгаж байна...");

  try {
    const msg = await invoke("apply_update");
    if (statusBox) statusBox.textContent = msg || "Шинэ хувилбар амжилттай сууж байна.";
    toast("Шинэ хувилбар сууж байна. Үйлчилгээ түр зогсож шинэчлэгдэнэ.");
  } catch (e) {
    if (statusBox) {
      statusBox.textContent = `Шинэчлэхэд алдаа гарлаа: ${e}`;
      statusBox.style.color = "#ef4444";
    }
    toast(String(e), "err");
  } finally {
    if (btn) btn.disabled = false;
    if (checkBtn) checkBtn.disabled = false;
    busy(false);
  }
});

boot().then(() => {
  initUpdatePage();
}).catch((e) => {
  document.body.innerHTML = `<pre style="padding:40px;color:#ef4444">Эхлүүлэхэд алдаа гарлаа:\n${e}</pre>`;
});
