// Prueba de la ventana del instalador (crates/anotalo/web/setup.html) en Chromium, sin WebView2 ni
// Rust, como run.mjs: sirve web/ en http://anotalo.test/ y cambia bridge.js por una API falsa con
// las formas de setup_ui.rs (`setup_info`, `setup_run`, `setup_close`). Hace capturas de cada
// estado para revisar cómo queda.
//
//   node crates/anotalo/tests/page/setup.mjs [carpeta de capturas]
//
// Sale con 1 si falla alguna comprobación.
import { readFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const WEB = resolve(HERE, "../../web");
const OUT = resolve(process.argv[2] || join(tmpdir(), "anotalo-setup-page"));
const PLAYWRIGHT = process.env.PLAYWRIGHT || "E:/Proyectos/Atomatizalo/web/node_modules/@playwright/test/index.mjs";
const { chromium } = await import(pathToFileURL(PLAYWRIGHT).href);

const ORIGIN = "http://anotalo.test/";
const TYPES = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".ttf": "font/ttf", ".svg": "image/svg+xml" };

// La API falsa: `info` la pone cada prueba en `window.__info` antes de cargar la página.
const FAKE = `(function () {
  window.__fake = { calls: [] };
  const answers = {
    setup_info: () => window.__info,
    setup_run: () => ({ ok: true }),
    setup_close: () => ({ ok: true }),
    log_js: () => ({ ok: true }),
  };
  const api = new Proxy({}, { get: (_, name) => typeof name === "string" && name !== "then"
    ? (...args) => { window.__fake.calls.push([name, args]); return Promise.resolve((answers[name] || (() => ({ ok: false })))(...args)); }
    : undefined });
  window.pywebview = { api };
  document.addEventListener("DOMContentLoaded", () => window.dispatchEvent(new Event("pywebviewready")));
})();`;

const browser = await chromium.launch();
const context = await browser.newContext({ viewport: { width: 460, height: 440 }, deviceScaleFactor: 2 });
const problems = [];
const results = [];
const shots = [];
await mkdir(OUT, { recursive: true });

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
function eq(a, b, what) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what}: ${JSON.stringify(a)} ≠ ${JSON.stringify(b)}`);
}

async function open(info) {
  const page = await context.newPage();
  page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") problems.push(`consola: ${m.text()}`); });
  page.on("pageerror", (e) => problems.push(`página: ${e.message}`));
  page.on("requestfailed", (r) => problems.push(`petición fallida: ${r.url()}`));
  await page.addInitScript((i) => { window.__info = i; }, info);
  await page.route(`${ORIGIN}**`, async (route) => {
    const path = decodeURIComponent(new URL(route.request().url()).pathname.slice(1)) || "setup.html";
    if (path === "bridge.js") return route.fulfill({ status: 200, contentType: "text/javascript", body: FAKE });
    const file = join(WEB, path);
    if (!resolve(file).startsWith(WEB)) return route.fulfill({ status: 403 });
    try {
      await route.fulfill({ status: 200, contentType: TYPES[extname(file)] || "application/octet-stream", body: await readFile(file) });
    } catch {
      problems.push(`no existe: ${path}`);
      await route.fulfill({ status: 404, body: "" });
    }
  });
  await page.goto(`${ORIGIN}setup.html`);
  await page.waitForFunction(() => document.querySelector(".pane:not([hidden])"));
  return page;
}
const visible = (page, sel) => page.$eval(sel, (e) => !e.hidden && !e.closest("[hidden]"));
const text = (page, sel) => page.$eval(sel, (e) => e.textContent.trim());
const calls = (page, m) => page.evaluate((m) => window.__fake.calls.filter(([n]) => n === m).map(([, a]) => a), m);
const push = (page, e) => page.evaluate((e) => window.onEvent(e), e);
async function shot(page, name) {
  await sleep(350);
  const file = join(OUT, `${name}.png`);
  await page.screenshot({ path: file });
  shots.push(file);
}
async function check(name, fn) {
  try { await fn(); results.push([true, name]); } catch (e) { results.push([false, name, e.message]); }
}

const base = { mode: "install", version: "1.0.0-alpha.3", installed: null, transcriptor: false, app_running: false,
  recordings: "C:\\Users\\Ana\\Documents\\Llamadas", options: { autostart: true, desktop: false } };

await check("instalar en un PC nuevo", async () => {
  const page = await open(base);
  eq(await text(page, "#i-title"), "Anotalo 1.0.0-alpha.3", "título");
  eq(await text(page, ".sub"), "Transcribe tus llamadas en tu PC. Nada sale de él.", "subtítulo");
  eq(await text(page, "#i-go"), "Instalar", "botón");
  eq(await page.$eval("#opt-autostart", (e) => e.checked), true, "Iniciar con Windows");
  eq(await page.$eval("#opt-desktop", (e) => e.checked), false, "escritorio");
  eq(await visible(page, "#note-transcriptor"), false, "nota de Transcriptor");
  eq(await visible(page, "#note-running"), false, "nota de app abierta");
  await shot(page, "1-instalar");
  await page.click("#opt-desktop");
  await page.click("#i-go");
  eq(await calls(page, "setup_run"), [[{ autostart: true, desktop: true }]], "setup_run");
  eq(await visible(page, "#i-progress"), true, "progreso");
  eq(await page.$eval("#i-go", (e) => e.disabled), true, "botón desactivado");
  eq(await page.$eval("#close", (e) => e.disabled), true, "cerrar desactivado");
  await push(page, { type: "setup", step: "closing" });
  await push(page, { type: "setup", step: "copying" });
  await shot(page, "2-instalando");
  await page.keyboard.press("Escape");
  eq(await calls(page, "setup_close"), [], "Esc no cierra mientras instala");
  await push(page, { type: "setup", step: "error", error: "no queda espacio en el disco" });
  eq(await text(page, "#i-error"), "No se pudo instalar: no queda espacio en el disco", "error");
  eq(await text(page, "#i-go"), "Reintentar", "reintentar");
  eq(await visible(page, "#i-progress"), false, "sin progreso tras el error");
  await shot(page, "3-error");
  await page.click("#i-go");
  eq((await calls(page, "setup_run")).length, 2, "reintento");
  eq(await visible(page, "#i-error"), false, "el error se va al reintentar");
  await push(page, { type: "setup", step: "done" });
  eq(await text(page, "#i-progress-text"), "Listo", "listo");
  await page.close();
});

await check("actualizar desde Transcriptor con la app abierta", async () => {
  const page = await open({ ...base, installed: "1.0.0-alpha.2", transcriptor: true, app_running: true,
    options: { autostart: false, desktop: true } });
  eq(await text(page, "#i-go"), "Actualizar", "botón");
  eq(await text(page, "#note-transcriptor"), "Sustituye a Transcriptor. Tus grabaciones y ajustes se conservan.", "nota");
  eq(await text(page, "#note-running"), "Anotalo se cerrará para actualizarse; si está grabando, la grabación se guarda antes.", "nota");
  eq(await page.$eval("#opt-autostart", (e) => e.checked), false, "inicio como estaba");
  eq(await page.$eval("#opt-desktop", (e) => e.checked), true, "escritorio como estaba");
  await shot(page, "4-actualizar");
  await page.close();
});

await check("reinstalar la misma versión, o una anterior", async () => {
  const same = await open({ ...base, installed: "1.0.0-alpha.3" });
  eq(await text(same, "#i-go"), "Reinstalar", "misma versión");
  const order = await same.evaluate(() => {
    const c = window.__setup.compareVersions;
    return [c("1.0.0", "1.0.0-alpha.3") > 0, c("1.0.0-alpha.10", "1.0.0-alpha.2") > 0, c("1.0.0-beta", "1.0.0-alpha.10") > 0,
      c("1.0.1", "1.0.0") > 0, c("1.0.0-alpha.2", "1.0.0-alpha.3") < 0, c("1.0.0", "1.0.0") === 0];
  });
  eq(order, [true, true, true, true, true, true], "orden SemVer");
  await same.close();
  const older = await open({ ...base, installed: "1.0.0" });
  eq(await text(older, "#i-go"), "Instalar", "instalada una más nueva");
  await older.close();
});

await check("desinstalar", async () => {
  const page = await open({ ...base, mode: "uninstall", options: { autostart: false, desktop: false } });
  eq(await visible(page, "#install"), false, "sin la parte de instalar");
  eq(await page.$eval("#uninstall h1", (e) => e.textContent), "Desinstalar Anotalo", "título");
  eq(await text(page, "#u-text"), "Tus grabaciones de C:\\Users\\Ana\\Documents\\Llamadas no se borran.", "texto");
  eq(await text(page, "#opt-purge + span"), "Borrar también los ajustes y los modelos descargados", "casilla");
  eq(await page.$eval("#opt-purge", (e) => e.checked), false, "casilla desmarcada");
  eq([await text(page, "#u-cancel"), await text(page, "#u-go")], ["Cancelar", "Desinstalar"], "botones");
  await shot(page, "5-desinstalar");
  await page.click("#u-go");
  eq(await calls(page, "setup_run"), [[{ purge: false }]], "setup_run");
  await shot(page, "6-desinstalando");
  await push(page, { type: "setup", step: "done" });
  eq(await text(page, "#u-done"), "Anotalo se ha desinstalado.", "hecho");
  eq(await visible(page, "#u-close"), true, "Cerrar");
  eq(await visible(page, "#u-go"), false, "sin Desinstalar");
  await shot(page, "7-desinstalado");
  await page.click("#u-close");
  eq((await calls(page, "setup_close")).length, 1, "setup_close");
  await page.close();
});

await check("cancelar y la X cierran", async () => {
  const page = await open({ ...base, mode: "uninstall" });
  await page.click("#u-cancel");
  await page.click("#close");
  await page.keyboard.press("Escape");
  eq((await calls(page, "setup_close")).length, 3, "setup_close");
  await page.close();
});

await browser.close();
let failed = false;
for (const [ok, name, why] of results) {
  console.log(`${ok ? "ok  " : "FALLA"} ${name}${why ? `: ${why}` : ""}`);
  failed ||= !ok;
}
if (problems.length) { failed = true; console.log("Problemas:\n  " + problems.join("\n  ")); }
console.log(`Capturas en ${OUT}:\n  ${shots.join("\n  ")}`);
process.exit(failed ? 1 : 0);
