// Ventana principal. Habla con Python vía window.pywebview.api; Python avisa con window.onEvent().
const S = { view: "recordings", kind: "all", query: "", selected: null, items: [], settings: {}, state: {} };
const $ = (sel, root = document) => root.querySelector(sel);
const $$ = (sel, root = document) => [...root.querySelectorAll(sel)];
const api = () => window.pywebview.api;

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
  S.view = view;
  $$(".nav").forEach((b) => b.dataset.view === view ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current"));
  $$(".view").forEach((v) => v.classList.toggle("on", v.id === "view-" + view));
}
$$(".nav").forEach((b) => b.addEventListener("click", () => show(b.dataset.view)));

// --- lista --------------------------------------------------------------
async function loadList() {
  S.items = await api().list_recordings(S.kind, S.query);
  if (S.kind === "all" && !S.query) $("#count").textContent = S.items.length || "";
  renderList();
  if (!S.items.find((i) => i.id === S.selected)) select(S.items[0]?.id || null);
}
function renderList() {
  const box = $("#items");
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
      <span class="snip">${esc(it.snippet) || "<i>Sin texto todavía</i>"}</span></span></button>`;
  }
  box.innerHTML = html;
  $$(".item", box).forEach((b) => b.addEventListener("click", () => select(b.dataset.id)));
}
$("#items").addEventListener("keydown", (e) => {
  if (!["ArrowDown", "ArrowUp"].includes(e.key)) return;
  e.preventDefault();
  const i = S.items.findIndex((x) => x.id === S.selected);
  const next = S.items[Math.max(0, Math.min(S.items.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
  if (next) { select(next.id); $(`.item[data-id="${CSS.escape(next.id)}"]`)?.focus(); }
});
let qTimer;
$("#q").addEventListener("input", (e) => { clearTimeout(qTimer); qTimer = setTimeout(() => { S.query = e.target.value.trim(); loadList(); }, 200); });
$$(".seg button").forEach((b) => b.addEventListener("click", () => {
  S.kind = b.dataset.kind;
  $$(".seg button").forEach((x) => x.setAttribute("aria-pressed", x === b));
  loadList();
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
}
function lineHtml(l, fresh = false) {
  return `<div class="line${fresh ? " new" : ""}"><span class="ts">${esc(l.ts)}</span><div>
    <div class="who ${l.me ? "me" : "ot"}"><i></i>${esc(l.speaker)}</div><div class="txt">${highlight(l.text, S.query)}</div></div></div>`;
}
async function remove(id) {
  const res = await api().delete(id);
  if (!res.ok) { toast(esc(res.error)); return; }
  await loadList();
  const t = toast(`Grabación borrada <button id="undo">Deshacer</button>`, 5000);
  $("#undo", t).onclick = async () => { await api().undo(res.token); t.classList.remove("on"); await loadList(); select(id); };
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
  } else if (evt.type === "recordings") {
    loadList().then(() => { if (evt.select) select(evt.select); });
  }
};
function renderState(st) {
  S.state = { ...S.state, ...st };
  const labels = { idle: "Inactivo", armed: "Esperando llamada", recording: "Grabando" };
  $("#state-dot").className = "dot " + S.state.state;
  $("#state-text").textContent = labels[S.state.state] || "";
  $("#state-keys").innerHTML = keysHtml(S.state.hotkey);
  if (S.state.device) $("#device").textContent = S.state.device;
  if (S.state.version) $("#version").textContent = "Versión " + S.state.version;
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
  listening = on;
  const f = $("#keyfield");
  f.classList.toggle("listening", on);
  f.innerHTML = on
    ? `Pulsa la combinación… <span class="sr">Esc cancela</span>`
    : `${keysHtml(S.settings.hotkey)}<button class="edit" aria-label="Cambiar atajo" title="Cambiar">${icon("edit")}</button>`;
  if (!on) $(".edit", f).onclick = () => { renderKeyfield(true); $("#hotkey-err").textContent = ""; };
}
function specFromEvent(e) {
  let key = null;
  if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
  else if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (/^F\d{1,2}$/.test(e.code)) key = `<${e.code.toLowerCase()}>`;
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

// --- arranque -----------------------------------------------------------
window.addEventListener("pywebviewready", async () => {
  S.settings = await api().get_settings();
  renderSettings();
  renderState(await api().get_state());
  await loadList();
});
