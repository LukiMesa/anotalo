// API falsa para probar la página sin WebView2 ni Rust. run.mjs la sirve en lugar de bridge.js:
// define `window.pywebview.api` como lo hace bridge.js (un Proxy: cualquier nombre es una orden) y
// lanza `pywebviewready` en DOMContentLoaded. Responde con las formas JSON de
// crates/core/src/ui_api.rs a partir de unas grabaciones de ejemplo, y apunta cada llamada en
// `window.__fake.calls` para que la prueba vea qué pidió la página.
(function () {
  "use strict";
  const fold = (s) => String(s ?? "").normalize("NFKD").replace(/[\u0300-\u036f]/g, "").toLowerCase();
  const LIVE = "2026-11-12_1130_Teams.md";

  const settings = {
    auto_mode: true, autostart: false, copy_on_stop: true, detect_browsers: true, detect_discord: false,
    detect_slack: true, detect_teams: true, detect_whatsapp: true, detect_zoom: true,
    hotkey: "<ctrl>+<alt>+r", hotkey_mark: "<ctrl>+<alt>+m", label_me: "Yo", label_others: "Otros",
    language: "es", model: "large-v3-turbo", output_dir: "C:\\Users\\usuario\\Documents\\Anotalo",
    release_after_min: 10, show_indicator: true,
  };

  const L = (ts, speaker, text) => ({ ts, speaker, text, mark: false });
  const M = (ts) => ({ ts, speaker: "", text: "", mark: true });
  // id, kind, app, date_label, time, duration, full_date, start_end, call_title, client, lines
  const recs = [
    {
      id: LIVE, kind: "call", app: "Teams", date_label: "Hoy", time: "11:30", duration: "",
      full_date: "12 nov 2026", start_end: "11:30", call_title: "Planificación Q1", client: null,
      lines: [L("00:00:03", "Persona 1", "Buenos días, ¿empezamos?"), L("00:00:07", "Yo", "Sí, cuando quieras.")],
    },
    {
      id: "2026-11-12_1000_Teams.md", kind: "call", app: "Teams", date_label: "Hoy", time: "10:00", duration: "01:00",
      full_date: "12 nov 2026", start_end: "10:00 – 10:01", call_title: "Revisión de la web", client: "Acme",
      lines: [
        L("00:00:01", "María", "Hola, soy María. Te cuento lo que vimos con el equipo de diseño."),
        L("00:00:03", "Yo", "Encantado. Dime."),
        M("00:00:05"),
        L("00:00:06", "Persona 1", "Y yo Juan. Necesitamos cambiar el botón de contacto antes del viernes."),
        L("00:00:09", "María", "Empezamos por la portada y luego vemos el formulario."),
        L("00:00:14", "Yo", "Perfecto, lo apunto."),
      ],
    },
    {
      id: "2026-11-12_0830_Meet.md", kind: "call", app: "Meet", date_label: "Hoy", time: "08:30", duration: "24:10",
      full_date: "12 nov 2026", start_end: "08:30 – 08:54", call_title: "Kickoff del rediseño", client: "Bértoli",
      lines: [
        L("00:00:02", "Persona 1", "¿Me oís bien?"),
        L("00:00:04", "Persona 2", "Sí, perfecto."),
        M("00:12:30"),
        L("00:12:31", "Yo", "Entonces quedamos en entregar la propuesta el lunes."),
      ],
    },
    {
      id: "2026-11-11_0905_Meet.md", kind: "call", app: "Meet", date_label: "Ayer", time: "09:05", duration: "",
      full_date: "11 nov 2026", start_end: "09:05", call_title: null, client: null,
      lines: [L("00:00:02", "Otros", "Hola, ¿me oyes?")],
    },
    {
      id: "2026-11-10_1120_Nota.md", kind: "note", app: "", date_label: "10 nov", time: "11:20", duration: "01:48",
      full_date: "10 nov 2026", start_end: "11:20 – 11:21", call_title: null, client: null,
      lines: [L("00:00:01", "Yo", "Apuntar la factura de Acme."), L("00:00:04", "Yo", "Y llamar a María.")],
    },
    {
      id: "2026-10-23_1504_Teams.md", kind: "call", app: "Teams", date_label: "23 oct", time: "15:04", duration: "36:51",
      full_date: "23 oct 2026", start_end: "15:04 – 15:41", call_title: null, client: "acme",
      lines: [L("00:00:05", "Yo", "Hola, ¿qué tal?"), L("00:00:09", "Otros", "Necesito cambiar el botón de contacto.")],
    },
    {
      id: "2026-10-20_1600_Zoom.md", kind: "call", app: "Zoom", date_label: "20 oct", time: "16:00", duration: "1:02:15",
      full_date: "20 oct 2026", start_end: "16:00 – 17:02",
      call_title: "Seguimiento semanal del proyecto de migración de la plataforma de pagos", client: "Globex Corporation",
      lines: [L("00:00:10", "Persona 1", "Repasemos los pendientes de la semana."), M("00:20:00"), L("00:20:02", "Yo", "Esto es lo importante.")],
    },
  ];
  const voices = [{ name: "María", seconds: 750 }, { name: "Juan", seconds: 24 }];

  const me = () => settings.label_me;
  const people = (r) => [...new Set(r.lines.filter((l) => !l.mark && l.speaker !== me()).map((l) => l.speaker))];
  const snippet = (r) => {
    const said = r.lines.filter((l) => !l.mark);
    const ordered = [...said.filter((l) => l.speaker !== me()), ...said.filter((l) => l.speaker === me())];
    return ordered.map((l) => l.text).join(" ").slice(0, 160);
  };
  const summary = (r) => ({
    id: r.id, kind: r.kind, app: r.app, title: r.kind === "note" ? "Nota rápida" : "Llamada", date_label: r.date_label,
    time: r.time, duration: r.duration, snippet: snippet(r), live: r.id === LIVE, call_title: r.call_title, client: r.client,
  });
  const err = (error) => ({ ok: false, error });
  const OK = { ok: true };
  const find = (id) => recs.find((r) => r.id === id);
  const reserved = (name) => [settings.label_me, settings.label_others].some((x) => fold(x) === fold(name)) || /^persona \d+$/i.test(name);
  function validateName(name) {
    const n = String(name ?? "").trim();
    if (!n) return [null, "Nombre no válido: está vacío"];
    if ([...n].length > 40) return [null, "Nombre no válido: más de 40 caracteres"];
    if (/[*:[\]\u2028\u2029\u0000-\u001f]/.test(n)) return [null, "Nombre no válido: no puede llevar * : [ ] ni saltos de línea"];
    if (n.toLowerCase() === "marca") return [null, "Nombre no válido: «Marca» es el nombre de las marcas"];
    return [n, null];
  }

  const handlers = {
    list_recordings(kind = "all", query = "", client = null) {
      const q = fold(String(query || "").trim()), c = fold(String(client || "").trim());
      return recs
        .filter((r) => kind === "all" || r.kind === kind)
        .filter((r) => !q || [...r.lines.map((l) => l.text), r.call_title, r.client].some((t) => t && fold(t).includes(q)))
        .filter((r) => !c || (r.client && fold(r.client) === c))
        .map(summary);
    },
    get_recording(id) {
      const r = find(id);
      if (!r) return err("La grabación ya no existe");
      return {
        ...summary(r), ok: true, full_date: r.full_date, start_end: r.start_end, people: people(r),
        lines: r.lines.map((l) => ({ ts: l.ts, speaker: l.speaker, me: !l.mark && l.speaker === me(), text: l.text, mark: l.mark })),
      };
    },
    set_client(id, client) {
      const r = find(id);
      if (!r) return err("La grabación ya no existe");
      // la prueba pide una negativa con __fake.refuseNext (como si el archivo se estuviera grabando)
      if (id === LIVE || window.__fake.refuseNext) {
        window.__fake.refuseNext = false;
        return err("La llamada aún se está grabando; pon el cliente al colgar");
      }
      const c = String(client ?? "").trim();
      if ([...c].length > 80) return err("Cliente no válido: más de 80 caracteres");
      r.client = c || null;
      return { ok: true, client: r.client };
    },
    rename_person(id, from, to) {
      const r = find(id);
      if (!r) return err("La grabación ya no existe");
      if (id === LIVE) return err("La llamada aún se está grabando; ponle nombre al colgar");
      const [name, bad] = validateName(to);
      if (bad) return err(bad);
      const mine = r.lines.filter((l) => !l.mark && l.speaker === from);
      if (!mine.length) return err(`No hay líneas de «${from}» en ${id}`);
      mine.forEach((l) => { l.speaker = name; });
      const remembered = !reserved(name);
      if (remembered && !voices.some((v) => v.name === name)) voices.push({ name, seconds: 42 });
      return { ok: true, lines: mine.length, remembered };
    },
    list_voices() {
      return [...voices].sort((a, b) => fold(a.name).localeCompare(fold(b.name)))
        .map((v) => ({ name: v.name, minutes: Math.round(v.seconds / 6) / 10 }));
    },
    rename_voice(from, to) {
      const [name, bad] = validateName(to);
      if (bad) return err(bad);
      if (reserved(name)) return err(`Nombre no válido: «${name}» es una etiqueta de la app`);
      const v = voices.find((x) => x.name === from);
      if (!v) return err(`No hay ninguna voz guardada con el nombre «${from}»`);
      v.name = name;
      return OK;
    },
    forget_voice(name) {
      const i = voices.findIndex((v) => v.name === name);
      if (i >= 0) voices.splice(i, 1);
      return OK;
    },
    copy: () => OK,
    save_as: () => ({ ok: false, cancelled: true }),
    delete: (id) => (id === LIVE ? err("No se puede borrar mientras se está grabando") : { ok: true, token: `1700000000000__${id}` }),
    undo: () => OK,
    purge_trash: () => ({ ok: true, purged: 0 }),
    open_folder: () => OK,
    get_state: () => ({
      state: "recording", hotkey: settings.hotkey, device: "GPU · NVIDIA GeForce RTX 3060 (Vulkan)",
      version: "1.0.0-alpha.2", frozen: false, kind: "call", update: null,
    }),
    check_updates: () => err("Solo en la versión instalada"),
    install_update: () => err("Solo en la versión instalada"),
    toggle_call: () => OK,
    get_settings: () => ({ ...settings }),
    set_setting(key, value) {
      if (!(key in settings)) return err(`Ajuste desconocido: ${key}`);
      if (key === "autostart") return err("Solo en la versión instalada");
      settings[key] = value;
      return OK;
    },
    set_hotkey(spec) { return handlers.set_setting("hotkey", spec); },
    set_hotkey_mark(spec) {
      if (spec === settings.hotkey) return err("Ese atajo ya es el de grabar");
      return handlers.set_setting("hotkey_mark", spec);
    },
    begin_hotkey_capture: () => OK,
    end_hotkey_capture: () => OK,
    log_js: () => OK,
    window_minimize: () => OK,
    window_toggle_maximize: () => ({ ok: true, maximized: false }),
    window_close: () => OK,
    choose_folder: () => ({ ok: false, cancelled: true }),
    copy_diagnostics: () => OK,
  };

  const calls = [], unknown = [];
  window.__fake = { calls, unknown, recs, voices, settings, LIVE };
  function call(method, args) {
    calls.push([method, args]);
    const h = handlers[method];
    if (!h) {
      unknown.push(method);
      return Promise.resolve(err(`Orden desconocida: ${method}`));
    }
    // asíncrono como el puente de verdad, y una copia: la página no comparte objetos con la API
    const out = JSON.parse(JSON.stringify(h(...args)));
    return new Promise((resolve) => setTimeout(() => resolve(out), 5));
  }
  const api = new Proxy({}, {
    get: (_, name) => (typeof name === "string" && name !== "then" ? (...args) => call(name, args) : undefined),
  });
  window.pywebview = { api };
  document.addEventListener("DOMContentLoaded", () => {
    call("log_js", ["ready"]);
    window.dispatchEvent(new Event("pywebviewready"));
  });
})();
