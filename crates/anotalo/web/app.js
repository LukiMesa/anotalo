window.addEventListener("error", (e) => { try { window.pywebview.api.log_js(`${e.message} @ ${e.filename}:${e.lineno}`); } catch (_) {} });
window.addEventListener("unhandledrejection", (e) => { try { window.pywebview.api.log_js(String(e.reason)); } catch (_) {} });
// Ventana principal. Habla con Anotalo vía window.pywebview.api (bridge.js); Anotalo avisa con window.onEvent().
const S = { view: "recordings", kind: "all", query: "", client: null, clients: [], selected: null, items: [], settings: {}, state: {}, voices: [] };
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
const checked = (text) => `<span class="ok">${icon("check")}</span>${text}`;

// --- diálogo ------------------------------------------------------------
// Pide un nombre (si hay `value`) o una confirmación. `onOk` hace el cambio y devuelve el error, si
// lo hay: el diálogo sigue abierto y lo muestra. Resuelve con el texto (o true), o null si se cancela.
function ask({ title, body = "", value = null, okText = "Guardar", danger = false, onOk = async () => null }) {
  const dlg = $("#dlg");
  dlg.innerHTML = `<form method="dialog">
    <h3 id="dlg-title">${esc(title)}</h3>${body ? `<p>${body}</p>` : ""}
    ${value === null ? "" : `<input class="text" id="dlg-input" autocomplete="off" spellcheck="false" maxlength="80" aria-labelledby="dlg-title">`}
    <span class="err" id="dlg-err" role="alert"></span>
    <div class="dlg-actions"><button class="btn" type="button" id="dlg-cancel">Cancelar</button>
      <button class="btn ${danger ? "destroy" : "primary"}" type="submit" id="dlg-ok">${esc(okText)}</button></div>
  </form>`;
  const input = $("#dlg-input", dlg), okBtn = $("#dlg-ok", dlg);
  if (input) input.value = value;
  return new Promise((resolve) => {
    let busy = false;
    const close = (v) => { dlg.close(); resolve(v); };
    $("#dlg-cancel", dlg).onclick = () => close(null);
    dlg.oncancel = (e) => { e.preventDefault(); if (!busy) close(null); };  // Esc
    $("form", dlg).onsubmit = async (e) => {
      e.preventDefault();
      if (busy) return;
      const v = input ? input.value.trim() : true;
      busy = okBtn.disabled = true;
      const error = await onOk(v);
      busy = okBtn.disabled = false;
      if (!error) { close(v); return; }
      $("#dlg-err", dlg).textContent = error;
      (input || okBtn).focus();
    };
    dlg.showModal();
    if (input) { input.focus(); input.select(); } else $("#dlg-cancel", dlg).focus();
    enter(dlg, { scale: 0.98, duration: ms("--duration-base") });
  });
}

// --- navegación ---------------------------------------------------------
function show(view) {
  if (listening && view !== S.view) renderKeyfield(null);
  S.view = view;
  $$(".nav").forEach((b) => b.dataset.view === view ? b.setAttribute("aria-current", "page") : b.removeAttribute("aria-current"));
  $$(".view").forEach((v) => v.classList.toggle("on", v.id === "view-" + view));
  moveMarker();
  if (view === "calls") loadVoices();  // nombrar a alguien en una llamada guarda su voz
}
$$(".nav").forEach((b) => b.addEventListener("click", () => show(b.dataset.view)));

// --- lista --------------------------------------------------------------
// animate: "stagger" (carga inicial, filtro, búsqueda) | "new" (solo las filas nuevas) | false
let listGen = 0;
async function loadList(animate = "new") {
  const gen = ++listGen;
  // Con un cliente elegido, la lista sin ese filtro da los clientes de los chips.
  const [items, unfiltered] = await Promise.all([
    S.client ? api().list_recordings(S.kind, S.query, S.client) : api().list_recordings(S.kind, S.query),
    S.client ? api().list_recordings(S.kind, S.query) : null,
  ]);
  if (gen !== listGen) return;  // llegó una carga más reciente: esta ya no vale
  S.items = Array.isArray(items) ? items : [];
  S.clients = clientsOf(Array.isArray(unfiltered) ? unfiltered : S.items);
  if (S.kind === "all" && !S.query && !S.client) $("#count").textContent = S.items.length || "";
  renderClients();
  renderList(animate);
  if (!S.items.find((i) => i.id === S.selected)) select(S.items[0]?.id || null);
}
// Los clientes de la lista, sin repetir (mayúsculas y tildes aparte), más el elegido aunque ya no salga.
function clientsOf(items) {
  const seen = new Map();
  for (const c of [...items.map((i) => i.client), S.client]) if (c && !seen.has(fold(c))) seen.set(fold(c), c);
  return [...seen.values()].sort((a, b) => a.localeCompare(b, "es", { sensitivity: "base" }));
}
function renderClients() {
  const box = $("#clients");
  const sig = JSON.stringify(S.clients);
  if (box.dataset.sig !== sig) {
    const focused = box.contains(document.activeElement) ? document.activeElement.dataset.client : null;
    box.dataset.sig = sig;
    box.innerHTML = [["", "Todos"], ...S.clients.map((c) => [c, c])]
      .map(([c, label]) => `<button class="cf" data-client="${esc(c)}">${esc(label)}</button>`).join("");
    $$(".cf", box).forEach((b) => b.addEventListener("click", () => {
      S.client = b.dataset.client || null;
      renderClients();
      loadList("stagger");
    }));
    if (focused != null) $$(".cf", box).find((b) => b.dataset.client === focused)?.focus();
  }
  box.hidden = !S.clients.length;
  const cur = fold(S.client || "");
  $$(".cf", box).forEach((b) => b.setAttribute("aria-pressed", fold(b.dataset.client) === cur));
}
function renderList(animate = false) {
  const box = $("#items");
  const before = new Set($$(".item", box).map((b) => b.dataset.id));
  if (!S.items.length) {
    const why = S.query ? `<b>Sin resultados</b>Nada coincide con “${esc(S.query)}”${S.client ? ` en ${esc(S.client)}` : ""}.`
      : S.client ? `<b>Sin resultados</b>No hay grabaciones de ${esc(S.client)} aquí.`
      : "<b>Aún no hay grabaciones</b>Entra en una llamada o pulsa el atajo para una nota rápida.";
    box.innerHTML = `<div class="empty"><div>${why}</div></div>`;
    return;
  }
  let html = "", day = null;
  for (const it of S.items) {
    if (it.date_label !== day) { day = it.date_label; html += `<div class="day">${esc(day)}</div>`; }
    const chip = it.live ? `<span class="chip live">En curso</span>` : it.app ? `<span class="chip">${esc(it.app)}</span>` : "";
    const time = [it.time, it.duration].filter(Boolean).join(" · ");
    // Cliente y título de la reunión: la búsqueda también mira ahí, así se ve por qué coincide.
    const ctx = it.client || it.call_title ? `<span class="ctx">${it.client ? `<span class="cl">${highlight(it.client, S.query)}</span>` : ""}${it.call_title ? `<span class="ct">${highlight(it.call_title, S.query)}</span>` : ""}</span>` : "";
    html += `<button class="item" role="option" data-id="${esc(it.id)}" aria-selected="${it.id === S.selected}">
      <span class="ico">${icon(it.kind === "note" ? "mic" : "phone")}</span>
      <span class="it"><span class="top">${esc(it.title)}${chip}<span class="time">${esc(time)}</span></span>${ctx}
      <span class="snip">${esc(it.snippet) || (it.live ? "<i>Sin texto todavía</i>" : "<i>Sin texto</i>")}</span></span></button>`;
  }
  box.innerHTML = html;
  const rows = $$(".item", box);
  rows.forEach((b) => b.addEventListener("click", () => select(b.dataset.id)));
  if (animate === "stagger") {
    const step = ms("--stagger-step");
    // Las 8 primeras en cascada; el resto entra con la última, para no aparecer antes que las de arriba.
    rows.forEach((b, i) => enter(b, { delay: Math.min(i, 7) * step }));
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
// quiet: volver a pintar la misma grabación tras un cambio (sin animar y sin perder el scroll).
async function select(id, quiet = false) {
  S.selected = id;
  $$(".item").forEach((b) => b.setAttribute("aria-selected", b.dataset.id === id));
  const d = $("#detail");
  if (!id) { d.innerHTML = `<div class="empty"><div><div class="ico">${icon("mic")}</div><b>Elige una grabación</b>Aquí verás la transcripción completa.</div></div>`; return; }
  const r = await api().get_recording(id);
  if (S.selected !== id) return;  // mientras tanto se eligió otra
  if (!r.ok) { d.innerHTML = `<div class="empty"><div><b>No disponible</b>${esc(r.error)}</div></div>`; return; }
  const title = r.kind === "note" ? "Nota rápida" : r.app === "Manual" ? "Llamada manual" : `Llamada de ${r.app || "—"}`;
  const scroll = quiet ? $("#lines")?.scrollTop || 0 : 0;
  const people = r.people || [];
  d.innerHTML = `
    <div class="dhead">
      <div><h2>${esc(title)}</h2>${r.call_title ? `<div class="ctitle">${esc(r.call_title)}</div>` : ""}<div class="meta"><span>${esc(r.full_date)}</span><span>${esc(r.start_end)}</span><span>${esc(r.duration)}</span></div></div>
      <div class="actions">
        <button class="btn primary" id="act-copy">${icon("copy")}Copiar</button>
        <button class="btn" id="act-save">${icon("download")}Descargar .md</button>
        <button class="btn icon danger" id="act-del" aria-label="Borrar grabación" title="Borrar" ${r.live ? "disabled" : ""}>${icon("trash")}</button>
      </div>
      <dl class="facts">
        <dt>Cliente</dt><dd id="client-field"></dd>
        <dt${people.length ? "" : " hidden"}>Personas</dt><dd class="people" id="people"${people.length ? "" : " hidden"}>${people.map((p) => personHtml(p, r.live)).join("")}</dd>
      </dl>
    </div>
    <div class="body" id="lines">${r.lines.map((l) => lineHtml(l)).join("") || `<div class="txt" id="nolines">${r.live ? "Esperando la primera frase…" : "Esta grabación no tiene texto."}</div>`}</div>`;
  renderClient(r);
  $$(".person[data-name]", d).forEach((b) => b.addEventListener("click", () => renamePerson(id, b.dataset.name)));
  $("#act-copy").onclick = async () => { const res = await api().copy(id); toast(res.ok ? checked("Copiado al portapapeles") : esc(res.error)); };
  $("#act-save").onclick = async () => { const res = await api().save_as(id); if (res.ok) toast(checked("Guardado")); else if (!res.cancelled) toast(esc(res.error)); };
  $("#act-del").onclick = () => remove(id);
  if (quiet) $("#lines").scrollTop = scroll;
  else enter(d, { y: 0, duration: ms("--duration-fast") });
}
function lineHtml(l, fresh = false) {
  if (l.mark) return `<div class="line marked${fresh ? " new" : ""}"><span class="ts">${esc(l.ts)}</span><div class="mk">${icon("flag")}Marca</div></div>`;
  return `<div class="line${fresh ? " new" : ""}"><span class="ts">${esc(l.ts)}</span><div>
    <div class="who ${l.me ? "me" : "ot"}"><i></i>${esc(l.speaker)}</div><div class="txt">${highlight(l.text, S.query)}</div></div></div>`;
}

// Personas de la llamada: pulsar una le cambia el nombre en toda la llamada (en directo, al colgar).
function personHtml(name, live) {
  if (live) return `<span class="person static" title="Podrás ponerle nombre al colgar">${icon("user")}<span>${esc(name)}</span></span>`;
  return `<button class="person" data-name="${esc(name)}" title="Cambiar el nombre" aria-label="${esc(name)}: cambiar el nombre">${icon("user")}<span>${esc(name)}</span>${icon("edit")}</button>`;
}
async function renamePerson(id, from) {
  let res = null;
  const to = await ask({
    title: "Nombre de la persona",
    body: `Cambia «${esc(from)}» en toda esta llamada. Si guardé su voz, la reconoceré en las próximas llamadas.`,
    value: from,
    onOk: async (v) => {
      res = null;
      if (v === from) return null;
      res = await api().rename_person(id, from, v);
      return res.ok ? null : res.error || "No se pudo cambiar el nombre";
    },
  });
  if (!to || !res?.ok) return;
  toast(checked(`«${esc(from)}» ahora es «${esc(to)}».${res.remembered ? " Recordaré su voz." : ""}`), 5000);
  if (res.remembered) loadVoices();
  await select(id, true);
  // el botón que se pulsó ya no existe: el foco pasa a la persona con su nombre nuevo
  if (document.activeElement === document.body) $$(".person[data-name]").find((b) => b.dataset.name === to)?.focus();
  loadList(false);  // el fragmento de la lista también lleva los nombres
}

// Cliente: se edita en su sitio. Enter guarda, Esc cancela; pulsar fuera del campo también guarda.
function renderClient(r) {
  const f = $("#client-field");
  if (r.live) {
    f.innerHTML = r.client ? `<span class="cval">${esc(r.client)}</span>` : `<span class="hint">Podrás ponerlo al colgar</span>`;
    return;
  }
  f.innerHTML = `<button class="cbtn${r.client ? "" : " empty"}" id="client-btn" title="Cambiar el cliente" aria-label="${r.client ? `Cliente: ${esc(r.client)}. Cambiar` : "Añadir cliente"}">${r.client ? esc(r.client) : "Añadir cliente"}${icon("edit")}</button>`;
  $("#client-btn").onclick = () => editClient(r);
}
function editClient(r) {
  const f = $("#client-field");
  f.innerHTML = `<input class="text cinput" id="client-input" maxlength="80" autocomplete="off" spellcheck="false" placeholder="Nombre del cliente" aria-label="Cliente" aria-describedby="client-hint"><span class="hint" id="client-hint">Enter guarda · Esc cancela</span>`;
  const input = $("#client-input");
  input.value = r.client || "";
  input.focus();
  input.select();
  let finished = false;
  const finish = async (keep, refocus) => {
    if (finished) return;
    finished = true;
    const v = input.value.trim();
    if (keep && v !== (r.client || "")) {
      input.disabled = true;
      const res = await api().set_client(r.id, v);
      if (res.ok) {
        r.client = res.client ?? null;
        const it = S.items.find((i) => i.id === r.id);
        if (it) it.client = r.client;
        loadList(false);  // los chips de clientes y la fila de la lista
      } else toast(esc(res.error || "No se pudo guardar el cliente"));
    }
    if (S.selected !== r.id || !f.isConnected) return;
    renderClient(r);
    if (refocus) $("#client-btn")?.focus();
  };
  input.addEventListener("keydown", (e) => {
    if (e.key === "Enter") { e.preventDefault(); finish(true, true); }
    else if (e.key === "Escape") { e.preventDefault(); finish(false, true); }
  });
  // Cambiar de ventana no cuenta como salir del campo: al volver, el foco sigue aquí.
  input.addEventListener("blur", () => { if (document.hasFocus()) finish(true, false); });
}
async function remove(id) {
  const res = await api().delete(id);
  if (!res.ok) { toast(esc(res.error)); return; }
  listGen++;  // una carga lanzada antes del borrado traería de vuelta la fila
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
      if (!evt.mark && !evt.me && evt.speaker) addLivePerson(evt.speaker);
    }
    clearTimeout(listTimer); listTimer = setTimeout(loadList, 800);
  } else if (evt.type === "state") {
    renderState(evt);
  } else if (evt.type === "window") {
    setMaxIcon(evt.maximized);
  } else if (evt.type === "update") {
    renderUpdate(evt);
  } else if (evt.type === "recordings") {
    loadList().then(() => { if (evt.select) select(evt.select); });
  }
};
function addLivePerson(name) {
  const box = $("#people");
  if (!box || $$(".person", box).some((p) => p.textContent.trim() === name)) return;
  box.insertAdjacentHTML("beforeend", personHtml(name, true));
  box.hidden = box.previousElementSibling.hidden = false;
}
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
  // Botón "Grabar llamada": mientras se graba cualquier cosa, la para (igual que el atajo).
  const rec = S.state.state === "recording";
  const cb = $("#call-btn");
  cb.classList.toggle("primary", !rec);
  cb.classList.toggle("rec", rec);
  cb.disabled = S.state.state === "preparing";
  $("#call-btn-text").textContent = rec ? "Parar grabación" : "Grabar llamada";
  cb.title = rec ? "Para la grabación en curso" : "Graba tu micrófono y todo lo que suena en el equipo";
  $("[data-icon]", cb).innerHTML = icon(rec ? "stop" : "phone");
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
  $$("[data-select]").forEach((s) => { s.value = String(S.settings[s.dataset.select]); });
  $("#folder").textContent = S.settings.output_dir || "";
  renderKeyfield(null);
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
  else if (s.dataset.select !== "release_after_min") toast(`<span class="ok">${icon("check")}</span>Cargando el modelo en segundo plano`);
}));
$("#open-folder").onclick = () => api().open_folder();
$("#choose-folder").onclick = async () => {
  const res = await api().choose_folder();
  if (res.ok) { S.settings.output_dir = res.path; $("#folder").textContent = res.path; setErr("output_dir", ""); loadList(); }
  else if (!res.cancelled) setErr("output_dir", res.error);
};

// Captura de los atajos: el de grabar y el de marca usan el mismo campo.
const KEYFIELDS = {
  hotkey: { field: "#keyfield", err: "#hotkey-err", method: "set_hotkey", label: "Cambiar atajo", done: "Atajo actualizado" },
  hotkey_mark: { field: "#keyfield-mark", err: "#hotkey-mark-err", method: "set_hotkey_mark", label: "Cambiar atajo de marca", done: "Atajo de marca actualizado" },
};
let listening = null;  // el ajuste del atajo que se está capturando
function renderKeyfield(key) {
  if (key && key !== listening) api().begin_hotkey_capture();
  else if (!key && listening) api().end_hotkey_capture();
  listening = key;
  for (const [k, kf] of Object.entries(KEYFIELDS)) {
    const f = $(kf.field), on = k === key;
    const was = f.classList.contains("listening");
    f.classList.toggle("listening", on);
    f.innerHTML = on
      ? `Pulsa la combinación… <span class="sr">Esc cancela</span>`
      : `${keysHtml(S.settings[k])}<button class="edit" aria-label="${kf.label}" title="Cambiar">${icon("edit")}</button>`;
    if (!on) {
      $(".edit", f).onclick = () => { renderKeyfield(k); $(kf.err).textContent = ""; };
      // al terminar, el foco vuelve al botón (el que se pulsó ya no existe)
      if (was && document.activeElement === document.body) $(".edit", f).focus();
    }
  }
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
  if (e.key === "Escape") { renderKeyfield(null); return; }
  const spec = specFromEvent(e);
  if (!spec) return; // solo modificadores todavía
  const key = listening, kf = KEYFIELDS[key];
  const res = await api()[kf.method](spec);
  if (res.ok) {
    S.settings[key] = spec;
    if (key === "hotkey") renderState({ hotkey: spec });
    toast(checked(kf.done));
  } else $(kf.err).textContent = res.error;
  if (listening === key) renderKeyfield(null);
}, true);

// Voces guardadas: renombrar y olvidar (con confirmación).
async function loadVoices() {
  const v = await api().list_voices();
  S.voices = Array.isArray(v) ? v : [];
  renderVoices();
}
const minutes = (m) => m < 1 ? "Menos de 1 min de voz" : `${m.toLocaleString("es-ES", { maximumFractionDigits: 1 })} min de voz`;
function renderVoices() {
  const box = $("#voices");
  if (!S.voices.length) {
    box.innerHTML = `<div class="set"><div class="l"><span>Aún no hay voces guardadas. Pon nombre a una persona en el detalle de una llamada.</span></div></div>`;
    return;
  }
  box.innerHTML = S.voices.map((v) => `<div class="set voice"><div class="l"><b>${esc(v.name)}</b><span>${esc(minutes(v.minutes))}</span></div>
    <button class="btn icon" data-act="rename" data-name="${esc(v.name)}" aria-label="Cambiar el nombre de ${esc(v.name)}" title="Cambiar el nombre">${icon("edit")}</button>
    <button class="btn icon danger" data-act="forget" data-name="${esc(v.name)}" aria-label="Olvidar la voz de ${esc(v.name)}" title="Olvidar">${icon("trash")}</button></div>`).join("");
  $$("[data-act]", box).forEach((b) => b.addEventListener("click", () => (b.dataset.act === "rename" ? renameVoice : forgetVoice)(b.dataset.name)));
}
async function renameVoice(name) {
  const to = await ask({
    title: "Nombre de la voz",
    body: `Las próximas llamadas reconocerán a «${esc(name)}» con el nombre nuevo. Las transcripciones guardadas no cambian.`,
    value: name,
    onOk: async (v) => {
      if (v === name) return null;
      const res = await api().rename_voice(name, v);
      return res.ok ? null : res.error || "No se pudo cambiar el nombre";
    },
  });
  if (!to || to === name) return;
  toast(checked(`Ahora es «${esc(to)}»`));
  await loadVoices();
  $$("#voices [data-act=rename]").find((b) => b.dataset.name === to)?.focus();
}
async function forgetVoice(name) {
  const ok = await ask({
    title: `¿Olvidar la voz de ${name}?`,
    body: "Las próximas llamadas ya no la reconocerán. Las transcripciones guardadas no cambian.",
    okText: "Olvidar",
    danger: true,
    onOk: async () => {
      const res = await api().forget_voice(name);
      return res.ok ? null : res.error || "No se pudo olvidar la voz";
    },
  });
  if (!ok) return;
  toast(checked(`Voz de «${esc(name)}» olvidada`));
  await loadVoices();
  ($("#voices [data-act]") || $('.nav[data-view="calls"]')).focus();
}

$("#copy-diag").onclick = async () => {
  const res = await api().copy_diagnostics();
  toast(res.ok ? checked("Diagnóstico copiado al portapapeles") : esc(res.error || "No se pudo copiar"));
};

// --- barra propia ---------------------------------------------------------
$("#call-btn").onclick = async () => { const r = await api().toggle_call(); if (!r.ok) toast(esc(r.error)); };
// --- actualizaciones -------------------------------------------------------
function renderUpdate(u) {
  const text = $("#upd-text"), check = $("#upd-check"), inst = $("#upd-install");
  const note = $("#upd-note"), noteSub = $("#upd-note-sub"), noteBtn = $("#upd-note-btn");
  if (!u) {  // versión de desarrollo
    text.textContent = "Solo en la versión instalada";
    check.disabled = true; inst.hidden = true; note.hidden = true;
    return;
  }
  const v = u.version, pct = u.progress != null ? ` ${Math.round(u.progress * 100)} %` : "";
  const msg = {
    idle: `Tienes la versión ${u.current}.`,
    checking: "Buscando actualizaciones…",
    latest: `Tienes la última versión (${u.current}).`,
    available: `Versión ${v} disponible.`,
    downloading: `Descargando la versión ${v}…${pct}`,
    waiting: `La versión ${v} se instalará al terminar la grabación.`,
    installing: "Instalando… la app se cerrará y volverá a abrirse.",
    error: v ? `Versión ${v} disponible.` : `Tienes la versión ${u.current}.`,
  }[u.state] || "";
  text.textContent = msg;
  setErr("update", u.state === "error" ? u.error : "");
  const canInstall = !!v && (u.state === "available" || u.state === "error");
  check.disabled = ["checking", "downloading", "waiting", "installing"].includes(u.state);
  inst.hidden = !canInstall;
  inst.textContent = u.state === "error" ? "Reintentar" : "Actualizar";
  const show = !!v && ["available", "downloading", "waiting", "installing", "error"].includes(u.state);
  if (show && note.hidden) note.hidden = false;
  if (!show) note.hidden = true;
  $("#upd-note-title").textContent = `Versión ${v || ""} disponible`;
  noteSub.textContent = u.state === "available" ? "" : msg;
  noteBtn.hidden = !canInstall;
  noteBtn.textContent = u.state === "error" ? "Reintentar" : "Actualizar";
}
$("#upd-check").onclick = async () => { const r = await api().check_updates(); if (!r.ok) setErr("update", r.error); };
const doInstall = async () => { const r = await api().install_update(); if (!r.ok) toast(esc(r.error)); };
$("#upd-install").onclick = doInstall;
$("#upd-note-btn").onclick = doInstall;
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
window.addEventListener("blur", () => { if (listening) renderKeyfield(null); });
document.addEventListener("visibilitychange", () => { if (document.hidden && listening) renderKeyfield(null); });

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
  const st0 = await api().get_state();
  renderState(st0);
  renderUpdate(st0.update);
  await loadList("stagger");
  loadVoices();
});
