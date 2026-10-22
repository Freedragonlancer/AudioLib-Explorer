// server/audiolib-server/src/admin/admin.js: logic of the admin page (served as
// /admin/admin.js; strict CSP, so no inline scripts). Every change carries the
// X-AudioLib-Admin header the server requires against cross-site requests.
"use strict";

const $ = (id) => document.getElementById(id);
let current = null;
let currentUpdate = null;
let updateRestarting = false;

function toast(text, bad) {
  const box = $("toast");
  box.textContent = text;
  box.style.background = bad ? "#b22" : "#333";
  box.style.display = "block";
  clearTimeout(toast.timer);
  toast.timer = setTimeout(() => { box.style.display = "none"; }, bad ? 6000 : 2500);
}

async function api(method, path, body) {
  const init = { method, headers: { "X-AudioLib-Admin": "1" }, cache: "no-store" };
  if (body !== undefined) {
    init.headers["Content-Type"] = "application/json";
    init.body = JSON.stringify(body);
  }
  const response = await fetch("/admin/api/" + path, init);
  const text = await response.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch (_) { data = null; }
  if (!response.ok) {
    const message = data && data.message ? data.message : (response.status + " " + text);
    throw new Error(message);
  }
  return data;
}

function when(seconds) {
  if (!seconds) return "从未";
  return new Date(seconds * 1000).toLocaleString();
}

const SOURCES = { admin: "管理页", config: "config.toml", default: "默认值" };

function render(status) {
  current = status;
  $("version").textContent = status.version;
  $("library").textContent = status.library;
  $("warnings").textContent = (status.warnings || []).join("\n");

  const lanFromPage = ["localhost", "127.0.0.1", "[::1]"].includes(location.hostname) ? null : location.origin;
  $("conn-public").value = status.public_url || "（未设置，见下方「公网地址」）";
  $("conn-lan").value = status.lan_url || lanFromPage || ("http://<本机局域网 IP>:" + status.http_port);
  const tokenNote = status.token_count + " 个已生成令牌" + (status.env_token ? " + 环境变量令牌" : "");
  const modeText = {
    off: "off：都不需要令牌",
    external: "external：局域网地址不需要令牌；外网地址需要在客户端填写令牌（" + tokenNote + "）",
    all: "all：两种地址都需要令牌（" + tokenNote + "）",
  }[status.auth_mode];
  $("conn-auth").textContent = modeText;

  if (document.activeElement !== $("public-url")) $("public-url").value = status.public_url || "";
  $("public-source").textContent = "公网地址来源：" + SOURCES[status.public_url_source] + "；局域网地址来源：" +
    ({ admin: "管理页", detected: "自动检测", none: "未检测到" }[status.lan_url_source]);
  if (document.activeElement !== $("lan-url")) $("lan-url").value = status.lan_url_source === "admin" ? status.lan_url : "";
  $("lan-candidates").textContent = status.lan_url_candidates.length
    ? "检测到：" + status.lan_url_candidates.map((c) => c.url + "（" + c.interface + "）").join("，")
    : "未检测到局域网 IPv4 地址";

  for (const radio of document.querySelectorAll("input[name=mode]")) radio.checked = radio.value === status.auth_mode;
  $("mode-source").textContent = "当前：" + status.auth_mode + "（来源：" + SOURCES[status.auth_mode_source] + "）";
  $("mode-warning").textContent =
    status.auth_mode !== "off" && status.token_count === 0 && !status.env_token
      ? (status.auth_mode === "all" ? "还没有任何令牌：所有客户端都会被拒绝（401）。" : "还没有任何令牌：外网客户端暂时都会被拒绝（401）。")
      : "";
  for (const radio of document.querySelectorAll("input[name=manage]")) radio.checked = radio.value === status.manage_mode;
  $("manage-source").textContent = "当前：" + status.manage_mode + "（来源：" + SOURCES[status.manage_mode_source] + "）";
  $("env-token").textContent = status.env_token ? " 另外，环境变量 AUDIOLIB_SERVER_TOKEN 中的令牌也有效（不在此列出）。" : "";

  const tls = status.tls;
  if (tls.configured && !["tls-cert", "tls-key", "tls-port"].includes(document.activeElement.id)) {
    $("tls-cert").value = tls.configured.cert_path;
    $("tls-key").value = tls.configured.key_path;
    $("tls-port").value = tls.configured.port;
  }
  const state = $("tls-state");
  state.className = tls.state === "listening" ? "ok" : (tls.state === "error" ? "warn" : "muted");
  state.textContent = {
    off: "未启用",
    starting: "启动中…",
    listening: "已在端口 " + tls.port + " 监听 HTTPS（来源：" + SOURCES[tls.source] + "）",
    error: "错误：" + (tls.message || ""),
  }[tls.state] || tls.state;

  $("pregen").checked = !!status.pregenerate_list_waveforms;
  if (document.activeElement !== $("lw-rate")) {
    $("lw-rate").value = status.list_waveform_rate_source === "admin" ? status.list_waveform_rate : "";
    $("lw-rate").placeholder = status.list_waveform_rate + "（当前生效值）";
  }
  $("lw-source").textContent = "预生成：" + (status.pregenerate_list_waveforms ? "开" : "关") + "（来源：" +
    SOURCES[status.pregenerate_list_waveforms_source] + "）；速率：" + status.list_waveform_rate + " 文件/秒（来源：" +
    SOURCES[status.list_waveform_rate_source] + "）";

  $("files").textContent = [status.settings_file, status.tokens_file].filter(Boolean).join("，");
}

function renderTokens(list) {
  const body = $("tokens");
  body.replaceChildren();
  if (!list.tokens.length) {
    const row = body.insertRow();
    const cell = row.insertCell();
    cell.colSpan = 4;
    cell.className = "muted";
    cell.textContent = "还没有生成令牌";
    return;
  }
  for (const token of list.tokens) {
    const row = body.insertRow();
    row.insertCell().textContent = token.name;
    row.insertCell().textContent = when(token.created_at);
    row.insertCell().textContent = when(token.last_used_at);
    const button = document.createElement("button");
    button.className = "danger";
    button.textContent = "撤销";
    button.addEventListener("click", async () => {
      if (!confirm("撤销令牌「" + token.name + "」？使用它的客户端会立即收到 401。")) return;
      try {
        await api("DELETE", "tokens/" + encodeURIComponent(token.id));
        toast("已撤销");
        await refresh();
      } catch (error) { toast(error.message, true); }
    });
    row.insertCell().appendChild(button);
  }
}

async function refresh() {
  try {
    render(await api("GET", "status"));
    renderTokens(await api("GET", "tokens"));
  } catch (error) { toast("读取状态失败：" + error.message, true); }
}

function renderUpdate(status) {
  currentUpdate = status;
  const active = ["checking", "downloading", "ready"].includes(status.state);
  $("update-status").textContent = "当前 " + status.current_version + (status.channel ? "（" + status.channel + "）" : "") + " · " + status.message;
  $("update-status").className = status.state === "error" ? "warn" : (status.state === "available" ? "ok" : "");
  $("update-check").disabled = active || status.state === "disabled";
  $("update-install").disabled = status.state !== "available";
  const release = $("update-release");
  release.classList.toggle("hidden", !status.release_url);
  if (status.release_url) release.href = status.release_url;
  if (status.state === "ready") updateRestarting = true;
}

async function refreshUpdate() {
  try {
    const status = await api("GET", "update");
    renderUpdate(status);
    if (updateRestarting && !["downloading", "ready"].includes(status.state)) {
      updateRestarting = false;
      toast("服务端已重新连接");
      await refresh();
    }
  } catch (error) {
    if (updateRestarting || (currentUpdate && currentUpdate.state === "downloading")) {
      updateRestarting = true;
      $("update-status").textContent = "服务端正在重启，等待重新连接…";
    } else {
      $("update-status").textContent = "读取更新状态失败：" + error.message;
    }
  }
}

$("update-check").addEventListener("click", async () => {
  $("update-check").disabled = true;
  try { renderUpdate(await api("POST", "update/check")); }
  catch (error) { toast(error.message, true); await refreshUpdate(); }
});
$("update-install").addEventListener("click", async () => {
  const update = currentUpdate;
  if (!update || update.state !== "available") return;
  if (!confirm("下载并安装服务端 " + update.tag + "？校验完成后会短暂断开连接并自动重启。请先确认客户端兼容此版本；资料库和设置会保留。")) return;
  $("update-install").disabled = true;
  try { renderUpdate(await api("POST", "update/install", { tag: update.tag })); }
  catch (error) { toast(error.message, true); await refreshUpdate(); }
});

async function change(method, path, body, done) {
  try {
    const status = await api(method, path, body);
    if (status && status.auth_mode) render(status);
    toast(done || "已保存");
    await refresh();
  } catch (error) { toast(error.message, true); }
}

async function copy(id) {
  const input = $(id);
  const text = input.value;
  try {
    await navigator.clipboard.writeText(text);
  } catch (_) {
    // Plain-HTTP LAN pages are not a secure context: fall back to a selection copy.
    input.select();
    document.execCommand("copy");
  }
  toast("已复制");
}

document.addEventListener("click", (event) => {
  const target = event.target.closest("[data-copy]");
  if (target) copy(target.dataset.copy);
});
$("save-public").addEventListener("click", () => change("PUT", "settings", { public_url: $("public-url").value }));
$("clear-public").addEventListener("click", () => change("PUT", "settings", { public_url: null }, "已清除"));
$("save-lan").addEventListener("click", () => change("PUT", "settings", { lan_url: $("lan-url").value }));
$("clear-lan").addEventListener("click", () => change("PUT", "settings", { lan_url: null }, "已改为自动检测"));
$("save-mode").addEventListener("click", () => {
  const picked = document.querySelector("input[name=mode]:checked");
  if (!picked) return;
  if (picked.value === "all" && !confirm("all 模式下局域网客户端也必须填写令牌，确定？")) return;
  change("PUT", "settings", { auth_mode: picked.value });
});
$("reset-mode").addEventListener("click", () => change("PUT", "settings", { auth_mode: null }, "已恢复默认"));
$("save-manage").addEventListener("click", () => {
  const picked = document.querySelector("input[name=manage]:checked");
  if (!picked) return;
  if (picked.value === "off" && !confirm("off 模式下所有客户端都不能扫描或移除记录，确定？")) return;
  change("PUT", "settings", { manage_mode: picked.value });
});
$("reset-manage").addEventListener("click", () => change("PUT", "settings", { manage_mode: null }, "已恢复默认"));
$("create-token").addEventListener("click", async () => {
  const name = $("token-name").value.trim();
  if (!name) { toast("请先填写名称", true); return; }
  try {
    const created = await api("POST", "tokens", { name });
    $("new-secret-name").textContent = created.token.name;
    $("new-secret-value").value = created.secret;
    $("new-secret").classList.remove("hidden");
    $("token-name").value = "";
    await refresh();
  } catch (error) { toast(error.message, true); }
});
$("hide-secret").addEventListener("click", () => {
  $("new-secret-value").value = "";
  $("new-secret").classList.add("hidden");
});
$("save-tls").addEventListener("click", () => change("PUT", "tls", {
  cert_path: $("tls-cert").value.trim(),
  key_path: $("tls-key").value.trim(),
  port: Number($("tls-port").value),
}, "已保存，见状态"));
$("disable-tls").addEventListener("click", () => {
  if (!confirm("停用原生 HTTPS 监听？")) return;
  change("DELETE", "tls", undefined, "已停用");
});

// ---- Library roots ----
let rootsList = [];
let scanTimer = null;
const numberText = (n) => (n === null || n === undefined ? "—" : Number(n).toLocaleString());

function cell(row, text, cls) {
  const td = row.insertCell();
  td.textContent = text;
  if (cls) td.className = cls;
  return td;
}

function renderRoots(list, scan) {
  rootsList = list;
  const body = $("roots");
  body.replaceChildren();
  if (!list.length) {
    cell(body.insertRow(), "还没有根目录：用下方「添加根目录」选择服务器上的文件夹。", "muted").colSpan = 7;
  }
  const busy = !!scan;
  for (const root of list) {
    const row = body.insertRow();
    cell(row, root.name);
    cell(row, root.server_path, "mono");
    const state = { online: "在线", checking: "检查中", waking: "唤醒中", offline: "离线" }[root.status] || root.status;
    cell(row, state + (root.scanning ? "（扫描中）" : "") + (root.status_message ? "：" + root.status_message : ""),
      root.status === "offline" ? "warn" : "");
    cell(row, numberText(root.file_count));
    cell(row, root.missing_count ? String(root.missing_count) : "0", root.missing_count ? "warn" : "");
    const last = root.last_scan;
    cell(row, last ? when(last.finished_at || last.started_at) + "（" + ({ completed: "完成", cancelled: "已取消", failed: "失败", running: "进行中" }[last.state] || last.state) +
      "，" + (last.mode === "full" ? "完整" : "增量") + "，新增 " + last.added + "）" : "从未");
    const actions = row.insertCell();
    for (const [label, mode] of [["增量扫描", "incremental"], ["完整扫描", "full"]]) {
      const button = document.createElement("button");
      button.textContent = label;
      button.disabled = busy;
      button.title = mode === "full" ? "读取每个文件；找不到的文件标记为缺失" : "只读取新增或变化的文件";
      button.addEventListener("click", () => startScan(root.name, mode));
      actions.appendChild(button);
    }
    const remove = document.createElement("button");
    remove.className = "danger";
    remove.textContent = "移除";
    remove.disabled = busy;
    remove.addEventListener("click", () => removeRoot(root));
    actions.appendChild(remove);
  }
}

function renderScan(scan) {
  const box = $("scan-box");
  if (!scan) { box.classList.add("hidden"); return; }
  box.classList.remove("hidden");
  $("scan-root").textContent = (scan.root || "?") + (scan.subpath ? "/" + scan.subpath : "") + "（" + (scan.mode === "full" ? "完整" : "增量") + "）";
  const text = scan.finalizing ? "正在收尾…" : scan.paused ? "已暂停" : scan.pause_requested ? "正在暂停…" : scan.cancel_requested ? "正在取消…" : "进行中";
  $("scan-state").textContent = text;
  const total = scan.discovered || 0;
  $("scan-bar").style.width = (total ? Math.min(100, (scan.processed / total) * 100) : 0) + "%";
  $("scan-detail").textContent = "已处理 " + numberText(scan.processed) + " / 发现 " + numberText(total) + " · 新增 " + scan.added +
    " · 更新 " + scan.updated + " · 未变 " + scan.unchanged + " · 错误 " + scan.errors + (scan.current_path ? "\n" + scan.current_path : "");
  $("scan-pause").disabled = scan.paused || scan.pause_requested || scan.cancel_requested;
  $("scan-resume").disabled = !(scan.paused || scan.pause_requested);
  $("scan-cancel").disabled = scan.cancel_requested;
  $("scan-box").dataset.session = scan.session_id;
}

async function refreshRoots() {
  try {
    const [list, current] = await Promise.all([api("GET", "roots"), api("GET", "scans/current")]);
    renderRoots(list, current.scan);
    renderScan(current.scan);
    clearTimeout(scanTimer);
    if (current.scan) scanTimer = setTimeout(refreshRoots, 1000);
  } catch (error) { $("roots-error").textContent = "读取根目录失败：" + error.message; }
}

async function startScan(root, mode) {
  $("roots-error").textContent = "";
  try {
    await api("POST", "scans", { root, mode });
    toast("扫描已开始");
  } catch (error) { $("roots-error").textContent = error.message; }
  await refreshRoots();
}

async function controlScan(action) {
  const id = $("scan-box").dataset.session;
  try { await api("POST", "scans/" + id + "/" + action); } catch (error) { toast(error.message, true); }
  await refreshRoots();
}
$("scan-pause").addEventListener("click", () => controlScan("pause"));
$("scan-resume").addEventListener("click", () => controlScan("resume"));
$("scan-cancel").addEventListener("click", () => {
  if (confirm("取消当前扫描？已处理的结果会保留。")) controlScan("cancel");
});

async function removeRoot(root) {
  if (!confirm("移除根目录「" + root.name + "」？\n\n将从库中删除它下面的 " + numberText(root.file_count) +
    " 条音频记录及相关的播放历史、歌单项、波形缓存。\n不会删除任何音频文件；重新添加并扫描即可恢复。")) return;
  $("roots-error").textContent = "";
  try {
    const done = await api("DELETE", "roots/" + encodeURIComponent(root.name));
    toast("已移除「" + root.name + "」：" + done.removed + " 条音频记录，" + done.folders_removed + " 个文件夹记录");
  } catch (error) { $("roots-error").textContent = error.message; }
  await refreshRoots();
}

function baseName(path) {
  const parts = path.split(/[\\/]+/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : "";
}

$("root-path").addEventListener("input", () => {
  if (!$("root-name").dataset.touched) $("root-name").value = baseName($("root-path").value.trim());
});
$("root-name").addEventListener("input", () => { $("root-name").dataset.touched = "1"; });
$("root-add").addEventListener("click", async () => {
  $("roots-error").textContent = "";
  const path = $("root-path").value.trim();
  const name = $("root-name").value.trim() || baseName(path);
  if (!path) { $("roots-error").textContent = "请先选择或填写服务器上的目录路径"; return; }
  try {
    await api("POST", "roots", { name, path });
    toast("已添加「" + name + "」；点击「增量扫描」把文件加入库");
    $("root-path").value = "";
    $("root-name").value = "";
    delete $("root-name").dataset.touched;
  } catch (error) { $("roots-error").textContent = error.message; }
  await refreshRoots();
});

// ---- Server disk browser ----
let browsing = null;

function link(text, onclick, title) {
  const a = document.createElement("a");
  a.textContent = text;
  if (title) a.title = title;
  a.addEventListener("click", onclick);
  return a;
}

function badge(text, cls) {
  const span = document.createElement("span");
  span.className = "badge" + (cls ? " " + cls : "");
  span.textContent = text;
  return span;
}

function crumbs(path) {
  const box = $("fs-crumbs");
  box.replaceChildren(link("起点", () => browse(null)));
  const unc = path.startsWith("\\\\");
  const drive = /^[A-Za-z]:/.test(path);
  const sep = unc || drive ? "\\" : "/";
  const parts = path.split(/[\\/]+/).filter(Boolean);
  let acc = unc ? "\\\\" : (drive ? "" : "/");
  if (!unc && !drive) box.append(" › ", link("/", () => browse("/")));
  parts.forEach((part, index) => {
    acc += part;
    const target = drive && index === 0 ? acc + sep : acc;
    if (index < parts.length - 1) acc += sep;
    box.append(" › ", index === parts.length - 1 ? document.createTextNode(part) : link(part, () => browse(target)));
  });
}

async function browse(path) {
  $("fs-error").textContent = "";
  try {
    const query = path === null ? "" : "?path=" + encodeURIComponent(path) + ($("fs-hidden").checked ? "&hidden=1" : "");
    const data = await api("GET", "fs" + query);
    const list = $("fs-list");
    list.replaceChildren();
    if (path === null) {
      browsing = null;
      $("fs-crumbs").replaceChildren(document.createTextNode("起点"));
      $("fs-info").textContent = "选择一个磁盘、卷或已有根目录开始浏览";
      $("fs-select").disabled = true;
      $("fs-up").disabled = true;
      const titles = { root: "文件系统根", volume: "已挂载的卷", home: "主目录", library_root: "现有根目录" };
      for (const place of data.places) {
        const row = document.createElement("div");
        row.append(link(place.name + "  " + (place.kind === "volume" ? "" : ""), () => browse(place.path), place.path));
        row.append(" ", badge(titles[place.kind] || place.kind, "link"));
        if (place.network) row.append(" ", badge("网络共享"));
        const small = document.createElement("span");
        small.className = "muted mono";
        small.textContent = "  " + place.path;
        row.append(small);
        list.appendChild(row);
      }
      return;
    }
    browsing = data;
    crumbs(data.path);
    $("fs-select").disabled = false;
    $("fs-up").disabled = !data.parent;
    $("fs-info").replaceChildren(document.createTextNode(
      "本目录直接包含 " + numberText(data.wav_files) + " 个 WAV 文件、" + numberText(data.other_files) + " 个其他文件" +
      (data.counts_truncated ? "（目录过大，计数不完整）" : "") + "；子目录 " + data.entries.length + (data.truncated ? "+（只显示前 " + data.entries.length + " 个）" : "") + " 个"));
    if (data.network) $("fs-info").append(" ", badge("网络共享（SMB 等）"));
    if (!data.entries.length) {
      const empty = document.createElement("div");
      empty.className = "muted";
      empty.textContent = "没有子目录";
      list.appendChild(empty);
    }
    for (const entry of data.entries) {
      const row = document.createElement("div");
      row.append(link("📁 " + entry.name, () => browse(entry.path), entry.path));
      if (entry.is_symlink) row.append(" ", badge("链接", "link"));
      list.appendChild(row);
    }
  } catch (error) {
    $("fs-error").textContent = error.message;
  }
}

function openBrowser() {
  $("modal").style.display = "flex";
  const typed = $("root-path").value.trim();
  browse(typed || null).then(() => { if ($("fs-error").textContent && typed) browse(null); });
}
$("root-browse").addEventListener("click", openBrowser);
$("fs-cancel").addEventListener("click", () => { $("modal").style.display = "none"; });
$("fs-home").addEventListener("click", () => browse(null));
$("fs-up").addEventListener("click", () => { if (browsing && browsing.parent) browse(browsing.parent); });
$("fs-hidden").addEventListener("change", () => { if (browsing) browse(browsing.path); });
$("fs-select").addEventListener("click", () => {
  if (!browsing) return;
  $("root-path").value = browsing.path;
  $("root-name").value = baseName(browsing.path) || "root";
  delete $("root-name").dataset.touched;
  $("modal").style.display = "none";
  $("root-name").focus();
});
$("modal").addEventListener("click", (event) => { if (event.target === $("modal")) $("modal").style.display = "none"; });

// ---- List waveform settings ----
async function saveListWaveforms(body) {
  try {
    render(await api("PUT", "settings", body));
    toast("已保存");
    refreshRuntime();
  } catch (error) { toast(error.message, true); }
}
$("save-lw").addEventListener("click", () => {
  const text = $("lw-rate").value.trim();
  const rate = text === "" ? null : Number(text);
  if (rate !== null && !(Number.isInteger(rate) && rate >= 1 && rate <= 1000)) {
    toast("速率必须是 1–1000 的整数", true);
    return;
  }
  saveListWaveforms({ pregenerate_list_waveforms: $("pregen").checked, list_waveform_rate: rate });
});
$("reset-lw").addEventListener("click", () => saveListWaveforms({ pregenerate_list_waveforms: null, list_waveform_rate: null }));

// ---- Runtime status (GET /admin/api/runtime) ----
const RUNTIME_INTERVAL_MS = 5000;
let runtimeTimer = null;

function bytesText(bytes) {
  if (bytes === null || bytes === undefined) return "未知";
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) { value /= 1024; unit += 1; }
  return (unit ? value.toFixed(1) : String(value)) + " " + units[unit];
}

function usage(used, limit) {
  if (used === null || used === undefined) return "未知 / " + bytesText(limit) + "（首次清理前未统计）";
  return bytesText(used) + " / " + bytesText(limit) + (limit ? "（" + Math.round((used / limit) * 100) + "%）" : "");
}

function renderRuntime(rt) {
  $("rt-updated").textContent = "更新于 " + new Date().toLocaleTimeString();
  const list = rt.list_waveforms;
  if (list) {
    const job = list.job;
    const progress = job
      ? "任务 " + ({ running: "进行中", paused: "已暂停", completed: "已完成", cancelled: "已取消", error: "出错", yielding: "让行中" }[job.state] || job.state) +
        "：已完成 " + numberText(job.done) + " / " + numberText(job.total) + "，待处理 " + numberText(list.pending ?? Math.max(0, job.total - job.done)) +
        "，失败 " + numberText(job.failed)
      : "无任务";
    $("rt-list").textContent = (list.enabled ? "已开启" : "已关闭") + (list.paused ? "（暂停）" : "") + " · " + progress +
      " · 解码失败记录 " + numberText(list.failed) + " · 按需队列 " + numberText(list.queued_on_demand) +
      " · 当前速率 " + list.current_rate + " 文件/秒（上限 " + list.rate + "）" +
      (list.last_error ? " · 最近错误：" + list.last_error.message : "");
  } else {
    $("rt-list").textContent = "库未就绪（" + rt.library + "）";
  }
  const slots = rt.slots;
  const names = { streams: "流", previews: "预览", transcodes: "转码", waveforms: "波形" };
  $("rt-slots").textContent = slots
    ? Object.keys(names).map((key) => names[key] + " " + slots[key].active + " / " + slots[key].max).join(" · ")
    : "—";
  $("rt-cache").textContent = list && rt.waveform_cache
    ? "波形缓存 " + usage(rt.waveform_cache.estimated_bytes, rt.waveform_cache.limit_bytes) + " · 列表波形缓存 " +
      usage(list.cache_bytes, list.cache_limit_bytes) + "，" + numberText(list.cache_entries) + " 条" + (list.over_budget ? "（已超限淘汰）" : "")
    : "—";
  const snap = rt.snapshot;
  $("rt-snapshot").textContent = snap
    ? "revision " + snap.revision + " · " + (snap.preparing || snap.preparing_zstd ? "正在生成快照" : "空闲") +
      (snap.cached ? " · 已缓存快照 r" + snap.cached.revision + "（" + bytesText(snap.cached.bytes) +
        (snap.cached.zstd_bytes ? "，zstd " + bytesText(snap.cached.zstd_bytes) : "") + "）" : " · 无缓存快照")
    : "—";
  const roots = $("rt-roots");
  roots.replaceChildren();
  if (!rt.roots.length) roots.textContent = "—";
  for (const root of rt.roots) {
    const line = document.createElement("div");
    line.className = root.online ? "" : "warn";
    line.textContent = root.name + "：" + ({ online: "在线", checking: "检查中", waking: "唤醒中", offline: "离线" }[root.state] || root.state) +
      "（最近检查 " + when(root.checked_at) + "）" + (root.message ? " " + root.message : "");
    roots.appendChild(line);
  }
  const up = rt.update;
  const skip = { disabled: "未启用自动更新", up_to_date: "已是最新", error: "检查出错" }[up.skip_reason];
  $("rt-update").textContent = (up.channel || "—") + " 通道 · 当前 " + up.current_version + " · 选中 " + (up.selected || "无") +
    (skip ? " · 跳过原因：" + skip : "") + " · " + up.message + " · 检查时间 " + when(up.checked_at);

  const limits = $("rt-limits");
  limits.replaceChildren();
  const rows = [];
  if (slots) {
    rows.push(["max_streams", slots.streams.max], ["max_previews", slots.previews.max],
      ["max_transcodes", slots.transcodes.max], ["max_waveforms", slots.waveforms.max]);
  }
  if (rt.waveform_cache) rows.push(["waveform_cache_mb", bytesText(rt.waveform_cache.limit_bytes)]);
  if (list) rows.push(["list_waveform_cache_mb", bytesText(list.cache_limit_bytes)]);
  for (const [name, value] of rows) {
    const row = limits.insertRow();
    row.insertCell().textContent = name;
    row.cells[0].className = "mono";
    row.insertCell().textContent = String(value);
    row.insertCell().appendChild(badge("需重启", "link"));
  }
}

async function refreshRuntime() {
  clearTimeout(runtimeTimer);
  runtimeTimer = null;
  if (document.visibilityState !== "visible") return;
  try {
    renderRuntime(await api("GET", "runtime"));
    $("rt-error").textContent = "";
  } catch (error) { $("rt-error").textContent = "读取运行状态失败：" + error.message; }
  if (document.visibilityState === "visible") runtimeTimer = setTimeout(refreshRuntime, RUNTIME_INTERVAL_MS);
}
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") refreshRuntime();
  else { clearTimeout(runtimeTimer); runtimeTimer = null; }
});

refresh();
refreshUpdate();
refreshRoots();
refreshRuntime();
setInterval(() => { if (document.visibilityState === "visible") refreshUpdate(); }, 2000);
setInterval(() => { if (document.visibilityState === "visible") { refresh(); if (!scanTimer || !$("scan-box").dataset.session || $("scan-box").classList.contains("hidden")) refreshRoots(); } }, 15000);
