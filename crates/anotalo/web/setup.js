// El instalador y el desinstalador (crates/anotalo/src/setup_ui.rs). Pide `setup_info`, lanza
// `setup_run` y sigue sus pasos con los avisos {type: "setup", step: …}.
(function () {
  "use strict";
  const $ = (id) => document.getElementById(id);
  const api = () => window.pywebview.api;
  const STEPS = ["closing", "copying", "shortcuts", "removing", "launching"];
  let info = null;
  let running = false;

  // Precedencia de SemVer (como core::updater::Version): >0 si a es posterior a b.
  function compareVersions(a, b) {
    const parse = (v) => {
      const [core, pre] = String(v).replace(/^v/i, "").split("+")[0].split(/-(.*)/s);
      return { nums: core.split(".").map(Number), pre: pre ? pre.split(".") : null };
    };
    const x = parse(a), y = parse(b);
    for (let i = 0; i < 3; i++) if (x.nums[i] !== y.nums[i]) return x.nums[i] - y.nums[i];
    if (!x.pre || !y.pre) return (x.pre ? -1 : 0) - (y.pre ? -1 : 0);
    for (let i = 0; i < Math.max(x.pre.length, y.pre.length); i++) {
      const p = x.pre[i], q = y.pre[i];
      if (p === undefined) return -1;
      if (q === undefined) return 1;
      const pn = /^\d+$/.test(p), qn = /^\d+$/.test(q);
      if (pn && qn && Number(p) !== Number(q)) return Number(p) - Number(q);
      if (pn !== qn) return pn ? -1 : 1;
      if (p !== q) return p < q ? -1 : 1;
    }
    return 0;
  }

  function installLabel(i) {
    if (!i.installed) return "Instalar";
    if (i.installed === i.version) return "Reinstalar";
    return compareVersions(i.version, i.installed) > 0 ? "Actualizar" : "Instalar";
  }

  function setRunning(on) {
    running = on;
    document.body.classList.toggle("running", on);
    $("close").disabled = on;
    for (const el of document.querySelectorAll("input, .buttons .btn")) el.disabled = on;
  }

  function showInstall() {
    $("install").hidden = false;
    $("i-title").textContent = `Anotalo ${info.version}`;
    $("opt-autostart").checked = !!info.options.autostart;
    $("opt-desktop").checked = !!info.options.desktop;
    $("note-transcriptor").hidden = !info.transcriptor;
    $("note-running").hidden = !info.app_running;
    $("i-go").textContent = installLabel(info);
    $("i-go").focus({ focusVisible: false });
  }

  function showUninstall() {
    $("uninstall").hidden = false;
    $("u-text").textContent = `Tus grabaciones de ${info.recordings} no se borran.`;
    $("u-cancel").focus({ focusVisible: false });
  }

  async function run(options, progress, error) {
    if (running) return;
    error.hidden = true;
    progress.hidden = false;
    setRunning(true);
    const r = await api().setup_run(options);
    if (!r || !r.ok) failed((r && r.error) || "Error interno");
  }

  function failed(message) {
    setRunning(false);
    if (info.mode === "uninstall") {
      $("u-progress").hidden = true;
      $("u-error").textContent = `No se pudo desinstalar: ${message}`;
      $("u-error").hidden = false;
      $("u-go").textContent = "Reintentar";
    } else {
      $("i-progress").hidden = true;
      $("i-error").textContent = `No se pudo instalar: ${message}`;
      $("i-error").hidden = false;
      $("i-go").textContent = "Reintentar";
    }
  }

  function done() {
    if (info.mode === "uninstall") {
      setRunning(false);
      $("u-progress").hidden = true;
      $("u-opts").hidden = true;
      $("u-done").hidden = false;
      $("u-go").hidden = true;
      $("u-cancel").hidden = true;
      $("u-close").hidden = false;
      $("u-close").focus({ focusVisible: false });
    } else {
      // la app se abre y esta ventana se cierra sola
      $("i-fill").style.width = "100%";
      $("i-progress-text").textContent = "Listo";
    }
  }

  window.onEvent = (e) => {
    if (!e || e.type !== "setup" || !info) return;
    if (e.step === "error") failed(e.error || "Error interno");
    else if (e.step === "done") done();
    else {
      const n = STEPS.indexOf(e.step);
      if (n >= 0) $("i-fill").style.width = `${Math.round(((n + 1) / (STEPS.length + 1)) * 100)}%`;
    }
  };

  function close() {
    if (!running) api().setup_close();
  }

  $("i-go").addEventListener("click", () =>
    run({ autostart: $("opt-autostart").checked, desktop: $("opt-desktop").checked }, $("i-progress"), $("i-error")));
  $("u-go").addEventListener("click", () =>
    run({ purge: $("opt-purge").checked }, $("u-progress"), $("u-error")));
  for (const id of ["close", "u-cancel", "u-close"]) $(id).addEventListener("click", close);
  document.addEventListener("keydown", (e) => { if (e.key === "Escape") close(); });

  window.addEventListener("pywebviewready", async () => {
    info = await api().setup_info();
    if (info.mode === "uninstall") showUninstall();
    else showInstall();
    document.title = info.mode === "uninstall" ? "Desinstalar Anotalo" : `Anotalo ${info.version}`;
  });

  window.__setup = { compareVersions, installLabel };
})();
