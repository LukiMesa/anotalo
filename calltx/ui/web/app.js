window.addEventListener("error", (e) => { try { window.pywebview.api.log_js(`${e.message} @ ${e.filename}:${e.lineno}`); } catch (_) {} });
window.addEventListener("unhandledrejection", (e) => { try { window.pywebview.api.log_js(String(e.reason)); } catch (_) {} });
// Ventana principal. Habla con Python vía window.pywebview.api; Python avisa con window.onEvent().
const S = { view: "recordings", kind: "all", query: "", selected: null, items: [], settings: {}, state: {} };
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const api = () => window.pywebview.api;

// --- movimiento ----------------------------------------------------------
// Duraciones y curvas salen de tokens.css. Con "Efectos de animación" desactivado en Windows,
// WebView2 informa prefers-reduced-motion y todo pasa a su estado final sin animar.
const reduceMotion = matchMedia("(prefers-reduced-motion: reduce)");
const motionOK = () => !reduceMotion.matches;
const token = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
const ms = (name) => parseFloat(token(name)) || 0;
function enter(el, { y = ms("--rise"), scale = 1, duration = ms("--duration-view"), delay = 0 } = {}) {
  if (!el || !motionOK()) return null;
  return el.animate(
    [{ opacity: 0, transform: `translateY(${y}px) scale(${scale})` }, { opacity: 1, transform: "none" }],
    { duration, delay, easing: token("--ease-out"), fill: "backwards" });
}
function moveMarker(instant = false) {
  const a = $('.nav[aria-current="page"]'), m = $(".nav-marker");
  if (!a || !m) return;
  m.classList.toggle("instant", instant || !motionOK());
  m.style.transform = `translateY(${a.offsetTop + (a.offsetHeight - m.offsetHeight) / 2}px)`;
}
function moveThumb(instant = false) {
  const b = $('.seg button[aria-pressed="true"]'), t = $(".seg-thumb");
  if (!b || !t || !b.offsetWidth) return;
  t.classList.toggle("instant", instant || !motionOK());
  t.style.width = b.offsetWidth + "px";
  t.style.transform = `translateX(${b.offsetLeft}px)`;
}

const esc = (s) => String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
const fold = (s) => s.normalize("NFKD").replace(/[̀-ͯ]/g, "").toLowerCase();
function highlight(text, q) {
  if (!q) return esc(text);
  const f = fold(text), fq = fold(q);
  let out = "", i = 0, j;
  while ((j = f.indexOf(fq, i)) !== -1) {
    out += esc(text.slice(i, j)) + "<mark>" + esc(text.slice(j, j + q.length)) + "</mark>";
    i = j + q.length;
  }
  return out + esc(text.slice(i));
}
function keysHtml(spec) {
  const names = { "<ctrl>": "Ctrl", "<alt>": "Alt", "<shift>": "Shift", "<cmd>": "Win", "<win>": "Win" };
  return (spec || "").split("+").map((p) => `<kbd>${esc(names[p] || p.replace(/[<>]/g, "").toUpperCase())}</kbd>`).join("");
}

// --- aviso --------------------------------------------------------------
let toastTimer;
function toast(html, ms = 3500) {
  const t = $("#toast");
  t.innerHTML = html;
  t.classList.add("on");
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => t.classList.remove("on"), ms);
  return t;
}

// --- navegación ---------------------------------------------------------
function show(view) {
  if (listening && view !== S.view) renderKeyfield(false);
  S.view = view;
  $$(".nav").forEach((b) => b.dataset.view === view ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current"));
  $$(".view").forEach((v) => v.classList.toggle("on", v.id === "view-" + view));
  moveMarker();
}
$$(".nav").forEach((b) => b.addEventListener("click", () => show(b.dataset.view)));

// --- lista --------------------------------------------------------------
// animate: "stagger" (carga inicial, filtro, búsqueda) | "new" (solo las filas nuevas) | false
let listGen = 0;
async function loadList(animate = "new") {
  const gen = ++listGen;
  const items = await api().list_recordings(S.kind, S.query);
  if (gen !== listGen) return;  // llegó una carga más reciente: esta ya no vale
  S.items = items;
  if (S.kind === "all" && !S.query) $("#count").textContent = S.items.length || "";
  renderList(animate);
  if (!S.items.find((i) => i.id === S.selected)) select(S.items[0]?.id || null);
}
function renderList(animate = false) {
  const box = $("#items");
  const before = new Set($$(".item", box).map((b) => b.dataset.id));
  if (!S.items.length) {
    box.innerHTML = `<div class="empty"><div>${S.query ? `<b>Sin resultados</b>Nada coincide con “${esc(S.query)}”.` : "<b>Aún no hay grabaciones</b>Entra en una llamada o pulsa el atajo para una nota rápida."}</div></div>`;
    return;
  }
  let html = "", day = null;
  for (const it of S.items) {
    if (it.date_label !== day) { day = it.date_label; html += `<div class="day">${esc(day)}</div>`; }
    const chip = it.live ? `<span class="chip live">En curso</span>` : it.app ? `<span class="chip">${esc(it.app)}</span>` : "";
    const time = [it.time, it.duration].filter(Boolean).join(" · ");
    html += `<button class="item" role="option" data-id="${esc(it.id)}" aria-selected="${it.id === S.selected}">
      <span class="ico">${icon(it.kind === "note" ? "mic" : "phone")}</span>
      <span class="it"><span class="top">${esc(it.title)}${chip}<span class="time">${esc(time)}</span></span>
      <span class="snip">${esc(it.snippet) || (it.live ? "<i>Sin texto todavía</i>" : "<i>Sin texto</i>")}</span></span></button>`;
  }
  box.innerHTML = html;
  const rows = $$(".item", box);
  rows.forEach((b) => b.addEventListener("click", () => select(b.dataset.id)));
  if (animate === "stagger") {
    const step = ms("--stagger-step");
    rows.slice(0, 8).forEach((b, i) => enter(b, { delay: i * step }));
  } else if (animate === "new" && before.size) {
    rows.filter((b) => !before.has(b.dataset.id)).forEach((b) => enter(b));
  }
}
function leave(el) {
  if (!el || !motionOK()) return Promise.resolve();
  return el.animate([{ opacity: 1, transform: "none" }, { opacity: 0, transform: "translateX(-8px) scale(.98)" }],
    { duration: ms("--duration-quick"), easing: token("--ease-in"), fill: "forwards" }).finished.catch(() => {});
}
$("#items").addEventListener("keydown", (e) => {
  if (!["ArrowDown", "ArrowUp"].includes(e.key)) return;
  e.preventDefault();
  const i = S.items.findIndex((x) => x.id === S.selected);
  const next = S.items[Math.max(0, Math.min(S.items.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
  if (next) { select(next.id); $(`.item[data-id="${CSS.escape(next.id)}"]`)?.focus(); }
});
let qTimer;
$("#q").addEventListener("input", (e) => { clearTimeout(qTimer); qTimer = setTimeout(() => { S.query = e.target.value.trim(); loadList("stagger"); }, 200); });
$$(".seg button").forEach((b) => b.addEventListener("click", () => {
  S.kind = b.dataset.kind;
  $$(".seg button").forEach((x) => x.setAttribute("aria-pressed", x === b));
  moveThumb();
  loadList("stagger");
}));

// --- detalle ------------------------------------------------------------
async function select(id) {
  S.selected = id;
  $$(".item").forEach((b) => b.setAttribute("aria-selected", b.dataset.id === id));
  const d = $("#detail");
  if (!id) { d.innerHTML = `<div class="empty"><div><div class="ico">${icon("mic")}</div><b>Elige una grabación</b>Aquí verás la transcripción completa.</div></div>`; return; }
  const r = await api().get_recording(id);
  if (!r.ok) { d.innerHTML = `<div class="empty"><div><b>No disponible</b>${esc(r.error)}</div></div>`; return; }
  const title = r.kind === "note" ? "Nota rápida" : `Llamada de ${r.app || "—"}`;
  d.innerHTML = `
    <div class="dhead">
      <div><h2>${esc(title)}</h2><div class="meta"><span>${esc(r.full_date)}</span><span>${esc(r.start_end)}</span><span>${esc(r.duration)}</span></div></div>
      <div class="actions">
        <button class="btn primary" id="act-copy">${icon("copy")}Copiar</button>
        <button class="btn" id="act-save">${icon("download")}Descargar .md</button>
        <button class="btn icon danger" id="act-del" aria-label="Borrar grabación" title="Borrar" ${r.live ? "disabled" : ""}>${icon("trash")}</button>
      </div>
    </div>
    <div class="body" id="lines">${r.lines.map((l) => lineHtml(l)).join("") || `<div class="txt" id="nolines">${r.live ? "Esperando la primera frase…" : "Esta grabación no tiene texto."}</div>`}</div>`;
  $("#act-copy").onclick = async () => { const res = await api().copy(id); toast(res.ok ? `<span class="ok">${icon("check")}</span>Copiado al portapapeles` : esc(res.error)); };
  $("#act-save").onclick = async () => { const res = await api().save_as(id); if (res.ok) toast(`<span class="ok">${icon("check")}</span>Guardado`); else if (!res.cancelled) toast(esc(res.error)); };
  $("#act-del").onclick = () => remove(id);
  enter(d, { y: 0, duration: ms("--duration-fast") });
}
function lineHtml(l, fresh = false) {
  return `<div class="line${fresh ? " new" : ""}"><span class="ts">${esc(l.ts)}</span><div>
    <div class="who ${l.me ? "me" : "ot"}"><i></i>${esc(l.speaker)}</div><div class="txt">${highlight(l.text, S.query)}</div></div></div>`;
}
async function remove(id) {
  const res = await api().delete(id);
  if (!res.ok) { toast(esc(res.error)); return; }
  await leave($(`.item[data-id="${CSS.escape(id)}"]`));
  await loadList(false);
  const t = toast(`Grabación borrada <button id="undo">Deshacer</button>`, 5000);
  let undone = false;
  $("#undo", t).onclick = async () => { undone = true; await api().undo(res.token); t.classList.remove("on"); await loadList(); select(id); };
  setTimeout(() => { if (!undone) api().purge_trash(5); }, 5600);
}

// --- eventos desde Python ------------------------------------------------
let listTimer;
window.onEvent = (evt) => {
  if (evt.type === "line") {
    if (evt.id === S.selected) {
      $("#nolines")?.remove();
      const box = $("#lines");
      if (box) {
        const atBottom = box.scrollTop + box.clientHeight >= box.scrollHeight - 40;
        box.insertAdjacentHTML("beforeend", lineHtml(evt, true));
        if (atBottom) box.scrollTop = box.scrollHeight;
      }
    }
    clearTimeout(listTimer); listTimer = setTimeout(loadList, 800);
  } else if (evt.type === "state") {
    renderState(evt);
  } else if (evt.type === "window") {
    setMaxIcon(evt.maximized);
  } else if (evt.type === "recordings") {
    loadList().then(() => { if (evt.select) select(evt.select); });
  }
};
let timerId = null;
function renderTimer() {
  const el = $("#state-timer");
  if (S.state.state !== "recording" || !S.state.started) { el.textContent = ""; return; }
  const s = Math.max(0, Math.floor(Date.now() / 1000 - S.state.started));
  el.textContent = `${String(Math.floor(s / 60)).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}
function renderState(st) {
  const was = S.state.state;
  S.state = { ...S.state, ...st };
  if (S.state.state === "recording" && was !== "recording") S.state.started = Date.now() / 1000;
  clearInterval(timerId);
  if (S.state.state === "recording") timerId = setInterval(renderTimer, 1000);
  renderTimer();
  const labels = { idle: "Inactivo", armed: "Esperando llamada", recording: "Grabando", preparing: "Preparando…" };
  $("#state-dot").className = "dot " + S.state.state;
  if (S.state.state !== was) enter($("#state-text"), { y: 0, duration: ms("--duration-fast") });
  const pct = S.state.progress != null ? ` · ${Math.round(S.state.progress * 100)} %` : "";
  $("#state-text").textContent = S.state.state === "preparing"
    ? `Preparando…${pct}` : (labels[S.state.state] || "") + (S.state.state !== "recording" ? pct : "");
  $("#state-keys").innerHTML = keysHtml(S.state.hotkey);
  if (S.state.device) $("#device").textContent = S.state.device;
  if (S.state.version) $("#version").textContent = "Versión " + S.state.version + (S.state.frozen ? "" : " · desarrollo");
  if (S.state.frozen === false) {
    const sw = $('[data-setting="autostart"]');
    sw.disabled = true;
    setErr("autostart", "Solo en la versión instalada");
  }
}

// --- ajustes ------------------------------------------------------------
function setErr(key, msg) { const el = $(`[data-err="${key}"]`); if (el) el.textContent = msg || ""; }
async function save(key, value) {
  const res = await api().set_setting(key, value);
  setErr(key, res.ok ? "" : res.error);
  if (res.ok) S.settings[key] = value;
  return res.ok;
}
function renderSettings() {
  $$("[data-setting]").forEach((b) => b.setAttribute("aria-checked", !!S.settings[b.dataset.setting]));
  $$("[data-text]").forEach((i) => { i.value = S.settings[i.dataset.text] ?? ""; });
  $$("[data-select]").forEach((s) => { s.value = S.settings[s.dataset.select]; });
  $("#folder").textContent = S.settings.output_dir || "";
  renderKeyfield(false);
}
$$("[data-setting]").forEach((b) => b.addEventListener("click", async () => {
  const key = b.dataset.setting, next = !S.settings[key];
  b.setAttribute("aria-checked", next);
  if (!(await save(key, next))) b.setAttribute("aria-checked", !next);
}));
$$("[data-text]").forEach((i) => i.addEventListener("change", async () => {
  const v = i.value.trim();
  if (!v) { i.value = S.settings[i.dataset.text]; return; }
  if (!(await save(i.dataset.text, v))) i.value = S.settings[i.dataset.text];
}));
$$("[data-select]").forEach((s) => s.addEventListener("change", async () => {
  if (!(await save(s.dataset.select, s.value))) s.value = S.settings[s.dataset.select];
  else toast(`<span class="ok">${icon("check")}</span>Cargando el modelo en segundo plano`);
}));
$("#open-folder").onclick = () => api().open_folder();
$("#choose-folder").onclick = async () => {
  const res = await api().choose_folder();
  if (res.ok) { S.settings.output_dir = res.path; $("#folder").textContent = res.path; setErr("output_dir", ""); loadList(); }
  else if (!res.cancelled) setErr("output_dir", res.error);
};

// Captura del atajo
let listening = false;
function renderKeyfield(on) {
  if (on !== listening) (on ? api().begin_hotkey_capture() : api().end_hotkey_capture());
  listening = on;
  const f = $("#keyfield");
  f.classList.toggle("listening", on);
  f.innerHTML = on
    ? `Pulsa la combinación… <span class="sr">Esc cancela</span>`
    : `${keysHtml(S.settings.hotkey)}<button class="edit" aria-label="Cambiar atajo" title="Cambiar">${icon("edit")}</button>`;
  if (!on) $(".edit", f).onclick = () => { renderKeyfield(true); $("#hotkey-err").textContent = ""; };
}
function specFromEvent(e) {
  // e.keyCode como respaldo: algunos teclados/entradas no rellenan e.code
  let key = null;
  const kc = e.keyCode;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (/^F\d{1,2}$/.test(e.code)) key = `<${e.code.toLowerCase()}>`;
  else if (kc >= 65 && kc <= 90) key = String.fromCharCode(kc).toLowerCase();
  else if (kc >= 48 && kc <= 57) key = String.fromCharCode(kc);
  else if (kc >= 112 && kc <= 135) key = `<f${kc - 111}>`;
  if (!key) return null;
  const mods = [e.ctrlKey && "<ctrl>", e.altKey && "<alt>", e.shiftKey && "<shift>", e.metaKey && "<cmd>"].filter(Boolean);
  return [...mods, key].join("+");
}
document.addEventListener("keydown", async (e) => {
  if (!listening) return;
  e.preventDefault();
  if (e.key === "Escape") { renderKeyfield(false); return; }
  const spec = specFromEvent(e);
  if (!spec) return; // solo modificadores todavía
  const res = await api().set_hotkey(spec);
  if (res.ok) { S.settings.hotkey = spec; renderState({ hotkey: spec }); toast(`<span class="ok">${icon("check")}</span>Atajo actualizado`); }
  else $("#hotkey-err").textContent = res.error;
  renderKeyfield(false);
}, true);

// --- barra propia ---------------------------------------------------------
$("#tb-min").onclick = () => api().window_minimize();
async function toggleMax() {
  setMaxIcon((await api().window_toggle_maximize()).maximized);
}
function setMaxIcon(maximized) {
  const r = { maximized };
  const changed = document.body.classList.contains("maximized") !== !!r.maximized;
  document.body.classList.toggle("maximized", !!r.maximized);
  $("#tb-max").setAttribute("aria-label", r.maximized ? "Restaurar" : "Maximizar");
  $("#tb-max").title = r.maximized ? "Restaurar" : "Maximizar";
  $("#tb-max").innerHTML = r.maximized
    ? '<svg class="i" viewBox="0 0 24 24" aria-hidden="true"><rect x="8" y="4" width="12" height="12" rx="1.5"/><path d="M16 16v2a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2v-8a2 2 0 0 1 2-2h2"/></svg>'
    : '<svg class="i" viewBox="0 0 24 24" aria-hidden="true"><rect x="6" y="6" width="12" height="12" rx="1.5"/></svg>';
  if (changed) enter($("#tb-max svg"), { y: 0, scale: 0.8, duration: ms("--duration-fast") });
}
$("#tb-max").onclick = toggleMax;
$("#tb-close").onclick = () => api().window_close();

// Si la ventana pierde el foco o se oculta mientras se captura un atajo, se cancela.
window.addEventListener("blur", () => { if (listening) renderKeyfield(false); });
document.addEventListener("visibilitychange", () => { if (document.hidden && listening) renderKeyfield(false); });

// --- arranque -----------------------------------------------------------
// Marcador de la barra lateral e indicador del filtro: elementos que se deslizan.
$(".side").prepend(Object.assign(document.createElement("span"), { className: "nav-marker" }));
$(".seg").prepend(Object.assign(document.createElement("span"), { className: "seg-thumb" }));
$$(".nav-marker, .seg-thumb").forEach((el) => el.setAttribute("aria-hidden", "true"));
moveMarker(true);
moveThumb(true);
new ResizeObserver(() => moveThumb(true)).observe($(".seg"));

window.addEventListener("pywebviewready", async () => {
  S.settings = await api().get_settings();
  renderSettings();
  renderState(await api().get_state());
  await loadList("stagger");
});
