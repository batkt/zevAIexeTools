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

const renderServices = () =>
  renderList($("#services-list"), "tpl-service", cfg.services, renderServices);
const renderLogs = () =>
  renderList($("#logs-list"), "tpl-log", cfg.logs.files, renderLogs);
const renderProbes = () =>
  renderList($("#probes-list"), "tpl-probe", cfg.health.http, renderProbes);

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
  server:   ["Сервер", "Хаана илгээх, ямар давтамжтай"],
  machine:  ["Энэ машин", "Хяналтын самбарт хэрхэн харагдах"],
  eventlog: ["Event Log", "Windows-ийн үйл явдлын бүртгэл"],
  services: ["Үйлчилгээ", "Хянах Windows үйлчилгээнүүд"],
  logfiles: ["Лог файл", "Програмын лог мөрүүд"],
  health:   ["Эрүүл мэнд", "CPU, RAM, диск ба HTTP шалгалт"],
  logview:  ["Бүртгэл", "zev-detector.log"],
};

function goto(page) {
  $$(".nav-item").forEach((b) => b.classList.toggle("active", b.dataset.page === page));
  $$(".page").forEach((s) => s.classList.toggle("active", s.dataset.page === page));
  const [title, sub] = PAGE_META[page];
  $("#page-title").textContent = title;
  $("#page-sub").textContent = sub;
  if (page === "logview") loadLog();
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

// ─── setup wizard ────────────────────────────────────────────────────────
const WIZ_LAST = 4;
let wizStep = 0;

function wizRender() {
  $$(".wizard-pane").forEach((p) => p.classList.toggle("active", +p.dataset.pane === wizStep));
  $$(".wizard-steps li").forEach((li) => {
    const n = +li.dataset.step;
    li.classList.toggle("current", n === wizStep);
    li.classList.toggle("done", n < wizStep);
  });
  $("#wiz-back").disabled = wizStep === 0;
  $("#wiz-next").textContent = wizStep === WIZ_LAST ? "Хадгалаад суулгах" : "Үргэлжлүүлэх";

  if (wizStep === WIZ_LAST) {
    const rows = [
      ["Сервер", cfg.server.url || "—"],
      ["Агент", cfg.agent.name || info.hostname],
      ["Байгууллага", cfg.agent.baiguullagiinNer || "—"],
      ["Хянах", [
        cfg.eventlog.enabled && "Event Log",
        cfg.logs.enabled && "лог файл",
        cfg.health.enabled && "эрүүл мэнд",
      ].filter(Boolean).join(", ") || "—"],
    ];
    $("#wiz-summary").innerHTML = "";
    for (const [k, v] of rows) {
      const row = document.createElement("div");
      const key = document.createElement("span");
      key.textContent = k;
      const val = document.createElement("b");
      val.textContent = v;
      row.append(key, val);
      $("#wiz-summary").appendChild(row);
    }
  }
}

/** Blocks moving past a step whose required field is empty. */
function wizValidate() {
  if (wizStep === 1 && !cfg.server.url.trim()) {
    toast("Серверийн хаяг заавал шаардлагатай.", "err");
    return false;
  }
  return true;
}

async function wizFinish() {
  const name = $("#wiz-service").value.trim();
  if (name && !cfg.services.some((s) => s.name === name)) {
    cfg.services.push({ name, label: name, must_run: true });
  }
  busy(true, "Тохиргоог хадгалж байна…");
  const ok = await save();
  busy(false);
  if (!ok) return;

  await serviceAction("install");
  const status = await invoke("service_status");
  if (status.installed) await serviceAction("start");

  closeWizard();
}

function closeWizard() {
  $("#wizard").hidden = true;
  $("#app").hidden = false;
  renderBindings();
  renderServices();
  renderLogs();
  renderProbes();
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

  // A machine with no config.toml has never been set up — walk the operator
  // through it instead of dropping them into a settings screen.
  const firstRun = !info.configExists;
  if (firstRun && !cfg.server.url) cfg.server.url = "https://admin.zevtabs.mn/api";

  renderBindings();

  if (firstRun) {
    $("#wizard").hidden = false;
    wizRender();
  } else {
    closeWizard();
  }

  statusTimer = setInterval(refreshStatus, 4000);
}

// ─── events ──────────────────────────────────────────────────────────────
$$(".nav-item").forEach((b) => b.addEventListener("click", () => goto(b.dataset.page)));
$$("[data-svc]").forEach((b) => b.addEventListener("click", () => serviceAction(b.dataset.svc)));

$("#save-btn").addEventListener("click", save);
$("#add-service").addEventListener("click", () => {
  cfg.services.push({ name: "", label: "", must_run: true });
  renderServices();
  markDirty();
});
$("#add-log").addEventListener("click", () => {
  cfg.logs.files.push({ path: "", glob: "", system: "", modul: "", pattern: "", ignore_pattern: "", context_lines: 8 });
  renderLogs();
  markDirty();
});
$("#add-probe").addEventListener("click", () => {
  cfg.health.http.push({ name: "", url: "", expect: "", timeout_secs: 5, system: "" });
  renderProbes();
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

$("#wiz-next").addEventListener("click", () => {
  if (!wizValidate()) return;
  if (wizStep === WIZ_LAST) return wizFinish();
  wizStep++;
  wizRender();
});
$("#wiz-back").addEventListener("click", () => {
  if (wizStep > 0) wizStep--;
  wizRender();
});
// Skip writes nothing: saving here would either fail silently or leave a
// half-filled config.toml behind, which would then stop the wizard from
// reappearing on the next launch. Settings and Save are right there instead.
$("#wiz-skip").addEventListener("click", closeWizard);

window.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "s") {
    e.preventDefault();
    if (isDirty()) save();
  }
});

boot().catch((e) => {
  document.body.innerHTML = `<pre style="padding:40px;color:#ef4444">Эхлүүлэхэд алдаа гарлаа:\n${e}</pre>`;
});
