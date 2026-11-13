// Prueba de la página (crates/anotalo/web) en Chromium, sin WebView2 ni Rust: sirve la carpeta en
// http://anotalo.test/ y cambia bridge.js por fake_api.js (la API de ui_api.rs con datos de ejemplo).
//
//   node crates/anotalo/tests/page/run.mjs [carpeta de capturas]
//
// Playwright no es una dependencia del proyecto: se importa de PLAYWRIGHT (por defecto, el de
// E:\Proyectos\Atomatizalo\web, que ya trae Chromium). Sale con 1 si falla alguna comprobación.
import { readFile, mkdir } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, extname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HERE = dirname(fileURLToPath(import.meta.url));
const WEB = resolve(HERE, "../../web");
const OUT = resolve(process.argv[2] || join(tmpdir(), "anotalo-page"));
const PLAYWRIGHT = process.env.PLAYWRIGHT || "E:/Proyectos/Atomatizalo/web/node_modules/@playwright/test/index.mjs";
const REDUCE = process.env.REDUCED_MOTION === "1";
const { chromium } = await import(pathToFileURL(PLAYWRIGHT).href);

const ORIGIN = "http://anotalo.test/";
const TYPES = { ".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".ttf": "font/ttf", ".woff2": "font/woff2", ".svg": "image/svg+xml", ".png": "image/png" };
const ACME = "2026-11-12_1000_Teams.md";
const LIVE = "2026-11-12_1130_Teams.md";

await mkdir(OUT, { recursive: true });
const browser = await chromium.launch();
const context = await browser.newContext({
  viewport: { width: 1110, height: 710 },  // 1120×720 menos el borde de 5 px de la ventana
  reducedMotion: REDUCE ? "reduce" : "no-preference",
});
const page = await context.newPage();

const problems = [];  // errores de consola, de la página y peticiones fallidas
page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") problems.push(`consola ${m.type()}: ${m.text()}`); });
page.on("pageerror", (e) => problems.push(`página: ${e.message}`));
page.on("requestfailed", (r) => problems.push(`petición fallida: ${r.url()}`));
await page.route(`${ORIGIN}**`, async (route) => {
  const path = decodeURIComponent(new URL(route.request().url()).pathname.slice(1)) || "index.html";
  const file = path === "bridge.js" ? join(HERE, "fake_api.js") : join(WEB, path);
  if (!resolve(file).startsWith(WEB) && path !== "bridge.js") return route.fulfill({ status: 403 });
  try {
    await route.fulfill({ status: 200, contentType: TYPES[extname(file)] || "application/octet-stream", body: await readFile(file) });
  } catch {
    problems.push(`no existe: ${path}`);
    await route.fulfill({ status: 404, body: "" });
  }
});

// --- utilidades --------------------------------------------------------------
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
async function until(fn, what, timeout = 3000) {
  const t0 = Date.now();
  let last;
  while (Date.now() - t0 < timeout) {
    try { last = await fn(); if (last) return last; } catch (e) { last = e.message; }
    await sleep(40);
  }
  throw new Error(`${what} (último valor: ${JSON.stringify(last)})`);
}
const texts = (sel) => page.$$eval(sel, (els) => els.map((e) => e.textContent.trim()));
const text = (sel) => page.$eval(sel, (e) => e.textContent.trim());
const calls = (method) => page.evaluate((m) => window.__fake.calls.filter(([n]) => n === m).map(([, a]) => a), method);
const settle = () => sleep(REDUCE ? 50 : 450);  // que terminen las animaciones antes de una captura
const shots = [];
async function shot(name) {
  await settle();
  const file = join(OUT, `${name}.png`);
  await page.screenshot({ path: file });
  shots.push(file);
}
const results = [];
async function check(name, fn) {
  try { await fn(); results.push([true, name]); }
  catch (e) { results.push([false, name, e.message]); }
}
function eq(a, b, what) {
  if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`${what}: ${JSON.stringify(a)} ≠ ${JSON.stringify(b)}`);
}

// --- comprobaciones ----------------------------------------------------------
await page.goto(`${ORIGIN}index.html`);

await check("la lista muestra todas las grabaciones", async () => {
  const n = await page.evaluate(() => window.__fake.recs.length);
  await until(async () => (await page.$$(".item")).length === n, `${n} filas`);
  eq(await text("#count"), String(n), "contador de la barra lateral");
  eq(await page.title(), "Anotalo", "título");
  eq(await text(".tb-brand"), "Anotalo", "marca");
  eq(await page.getAttribute("#q", "placeholder"), "Buscar en el texto, título o cliente", "pista de búsqueda");
  eq(await texts(".day"), ["Hoy", "Ayer", "10 nov", "23 oct", "20 oct"], "días");
});

await check("la grabación en curso: personas sin editar y marca en directo", async () => {
  await until(async () => (await page.$(".ctitle")) && (await text(".ctitle")) === "Planificación Q1", "título de la reunión en curso");
  eq(await texts(".person.static"), ["Persona 1"], "personas en curso");
  eq(await page.$$(".person[data-name]"), [], "sin botones de renombrar en curso");
  eq(await text("#client-field"), "Podrás ponerlo al colgar", "cliente en curso");
  await page.evaluate((id) => {
    window.onEvent({ type: "line", id, ts: "00:00:12", speaker: "Persona 2", me: false, text: "Hola a todos." });
    window.onEvent({ type: "line", id, ts: "00:00:15", speaker: "", me: false, text: "", mark: true });
  }, LIVE);
  await until(async () => (await page.$$(".line.marked")).length === 1, "marca en directo");
  eq(await texts(".person.static"), ["Persona 1", "Persona 2"], "persona nueva en directo");
  await shot("01-list");
});

await check("el detalle: título, cliente, personas y fila de marca", async () => {
  await page.click(`.item[data-id="${ACME}"]`);
  await until(async () => (await page.$("#client-btn")) && (await text(".ctitle")) === "Revisión de la web", "detalle de Acme");
  eq(await text("#client-btn"), "Acme", "cliente");
  eq(await texts(".person[data-name]"), ["María", "Persona 1"], "personas");
  eq(await texts(".facts dt"), ["Cliente", "Personas"], "etiquetas");
  eq(await texts(".line.marked .ts"), ["00:00:05"], "hora de la marca");
  eq(await texts(".line.marked .mk"), ["Marca"], "texto de la marca");
  const color = await page.$eval(".line.marked .mk", (e) => getComputedStyle(e).color);
  eq(color, "rgb(46, 197, 230)", "color de acento de la marca");
  await shot("02-detail");
});

await check("renombrar a una persona llama a rename_person y confirma «Recordaré su voz»", async () => {
  await page.click('.person[data-name="Persona 1"]');
  await until(() => page.$eval("#dlg", (d) => d.open), "diálogo abierto");
  eq(await page.inputValue("#dlg-input"), "Persona 1", "nombre propuesto");
  eq(await page.evaluate(() => document.activeElement.id), "dlg-input", "foco en el nombre");
  await page.fill("#dlg-input", "Juan");
  await shot("03-rename-dialog");
  await page.keyboard.press("Enter");
  await until(async () => !(await page.$eval("#dlg", (d) => d.open)), "diálogo cerrado");
  eq(await calls("rename_person"), [[ACME, "Persona 1", "Juan"]], "llamada a rename_person");
  await until(async () => (await text("#toast")).includes("Recordaré su voz"), "aviso «Recordaré su voz»");
  await until(async () => JSON.stringify(await texts(".person[data-name]")) === JSON.stringify(["María", "Juan"]), "personas tras renombrar");
  if (!(await texts(".who")).includes("Juan")) throw new Error("las líneas no muestran a Juan");
  eq(await page.evaluate(() => document.activeElement.dataset.name), "Juan", "el foco queda en la persona renombrada");
  await shot("03b-renamed");
});

await check("renombrar: un nombre no válido se queda en el diálogo; Esc cancela; una etiqueta no se recuerda", async () => {
  await page.click('.person[data-name="Juan"]');
  await until(() => page.$eval("#dlg", (d) => d.open), "diálogo abierto");
  await page.fill("#dlg-input", "Ju:an");
  await page.keyboard.press("Enter");
  await until(async () => (await text("#dlg-err")).startsWith("Nombre no válido"), "error en el diálogo");
  eq(await page.$eval("#dlg", (d) => d.open), true, "sigue abierto");
  await page.keyboard.press("Escape");
  await until(async () => !(await page.$eval("#dlg", (d) => d.open)), "Esc cierra");
  eq((await calls("rename_person")).length, 2, "solo el intento no válido llegó a la API");
  await page.click('.person[data-name="Juan"]');
  await page.fill("#dlg-input", "Persona 3");
  await page.keyboard.press("Enter");
  await until(async () => (await text("#toast")).includes("«Juan» ahora es «Persona 3»"), "aviso de cambio");
  if ((await text("#toast")).includes("Recordaré")) throw new Error("una etiqueta no debe decir «Recordaré su voz»");
});

await check("filtro por cliente: chips de los clientes y la lista se reduce", async () => {
  await until(async () => !(await page.$eval("#clients", (e) => e.hidden)), "chips visibles");
  eq(await texts("#clients .cf"), ["Todos", "Acme", "Bértoli", "Globex Corporation"], "chips (acme y Acme son el mismo)");
  eq(await page.getAttribute('#clients .cf[data-client=""]', "aria-pressed"), "true", "Todos activo");
  await page.click('#clients .cf[data-client="Acme"]');
  await until(async () => (await page.$$(".item")).length === 2, "2 grabaciones de Acme");
  const last = (await calls("list_recordings")).at(-2);  // la filtrada (la otra trae los chips)
  eq(last, ["all", "", "Acme"], "list_recordings con cliente");
  eq(await page.getAttribute('#clients .cf[data-client="Acme"]', "aria-pressed"), "true", "Acme activo");
  eq(await text("#count"), "7", "el contador no cambia con el filtro");
  await shot("04-client-filter");
  await page.click('#clients .cf[data-client="Bértoli"]');
  await until(async () => (await page.$$(".item")).length === 1, "1 grabación de Bértoli");
  await page.click('#clients .cf[data-client=""]');
  await until(async () => (await page.$$(".item")).length === 7, "todas otra vez");
});

await check("la búsqueda mira el texto, el título y el cliente", async () => {
  await page.fill("#q", "acme");
  await until(async () => (await page.$$(".item")).length === 3, "3 resultados para «acme»");
  if (!(await page.$(".it .ctx mark"))) throw new Error("sin resaltado en el cliente");
  await page.fill("#q", "migración");
  await until(async () => (await page.$$(".item")).length === 1, "1 resultado por título");
  await page.fill("#q", "");
  await until(async () => (await page.$$(".item")).length === 7, "sin búsqueda");
});

// una búsqueda que al quitar tildes queda vacía (una tilde suelta pegada) no cuelga la página
await check("una búsqueda solo de tildes sueltas no cuelga la página", () => Promise.race([
  (async () => {
    await page.fill("#q", "́");
    await sleep(300);
    await page.evaluate(() => 1);
    await page.fill("#q", "");
    await until(async () => (await page.$$(".item")).length === 7, "sin búsqueda");
  })(),
  sleep(5000).then(() => { throw new Error("la página se colgó"); }),
]));

await check("una nota: sin personas y con el cliente por añadir", async () => {
  await page.click('.item[data-id="2026-11-10_1120_Nota.md"]');
  await until(async () => (await page.$("#client-btn")) && (await text(".dhead h2")) === "Nota rápida", "detalle de la nota");
  eq(await text("#client-btn"), "Añadir cliente", "cliente vacío");
  eq(await texts(".facts dt:not([hidden])"), ["Cliente"], "sin fila de personas");
  eq(await page.$(".ctitle"), null, "sin título de reunión");
});

await check("el cliente se edita en su sitio con el teclado (Enter guarda, Esc cancela)", async () => {
  await page.click(`.item[data-id="${ACME}"]`);
  await until(async () => (await page.$("#client-btn")) && (await text(".ctitle")) === "Revisión de la web", "detalle de Acme");
  await page.focus("#client-btn");
  await page.keyboard.press("Enter");
  await until(() => page.evaluate(() => document.activeElement.id === "client-input"), "foco en el campo");
  await page.keyboard.type("Acme Corp");
  await shot("05-client-edit");
  await page.keyboard.press("Enter");
  await until(async () => (await page.$("#client-btn")) && (await text("#client-btn")) === "Acme Corp", "cliente guardado");
  eq(await calls("set_client"), [[ACME, "Acme Corp"]], "llamada a set_client");
  eq(await page.evaluate(() => document.activeElement.id), "client-btn", "el foco vuelve al botón");
  await until(async () => (await texts("#clients .cf")).includes("Acme Corp"), "chip del cliente nuevo");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Otra cosa");
  await page.keyboard.press("Escape");
  await until(async () => (await page.$("#client-btn")) && (await text("#client-btn")) === "Acme Corp", "Esc no cambia nada");
  eq((await calls("set_client")).length, 1, "Esc no llama a la API");
  // pulsar fuera del campo también guarda
  await page.keyboard.press("Enter");
  await page.keyboard.type("Acme SL");
  await page.click("#lines");
  await until(async () => (await page.$("#client-btn")) && (await text("#client-btn")) === "Acme SL", "guardado al salir");
  eq((await calls("set_client")).at(-1), [ACME, "Acme SL"], "set_client al salir del campo");
  // una negativa: aviso con el error y el valor de antes
  await page.evaluate(() => { window.__fake.refuseNext = true; });
  await page.focus("#client-btn");
  await page.keyboard.press("Enter");
  await page.keyboard.type("Rechazado");
  await page.keyboard.press("Enter");
  await until(async () => (await text("#toast")).includes("pon el cliente al colgar"), "aviso de la negativa");
  eq(await text("#client-btn"), "Acme SL", "vuelve al valor de antes");
});

await check("Ajustes: atajo de marca con la misma captura que el de grabar", async () => {
  await page.click('.nav[data-view="hotkey"]');
  await until(async () => (await texts("#keyfield-mark kbd")).join("+") === "Ctrl+Alt+M", "atajo de marca Ctrl+Alt+M");
  eq(await texts("#keyfield kbd"), ["Ctrl", "Alt", "R"], "atajo de grabar");
  await page.click("#keyfield-mark .edit");
  await until(() => page.$eval("#keyfield-mark", (e) => e.classList.contains("listening")), "escuchando");
  eq((await calls("begin_hotkey_capture")).length, 1, "pausa los atajos");
  await page.keyboard.press("Control+Alt+KeyR");  // ya es el de grabar: error
  await until(async () => (await text("#hotkey-mark-err")) === "Ese atajo ya es el de grabar", "error del atajo");
  await page.click("#keyfield-mark .edit");
  await page.keyboard.press("Control+Alt+KeyK");
  await until(async () => (await texts("#keyfield-mark kbd")).join("+") === "Ctrl+Alt+K", "atajo nuevo");
  eq((await calls("set_hotkey_mark")).map((a) => a[0]), ["<ctrl>+<alt>+r", "<ctrl>+<alt>+k"], "set_hotkey_mark");
  eq((await calls("set_hotkey")).length, 0, "no toca el atajo de grabar");
  eq((await calls("end_hotkey_capture")).length, 2, "devuelve los atajos");
  eq(await texts("#state-keys kbd"), ["Ctrl", "Alt", "R"], "la barra sigue con el de grabar");
  eq(await page.evaluate(() => document.activeElement.getAttribute("aria-label")), "Cambiar atajo de marca", "foco de vuelta en el botón");
  await shot("06-settings-hotkeys");
});

await check("Ajustes: voces guardadas con cambiar nombre y olvidar con confirmación", async () => {
  await page.click('.nav[data-view="calls"]');
  await until(async () => (await texts("#voices .voice b")).length === 2, "2 voces");
  eq(await texts("#voices .voice b"), ["Juan", "María"], "voces por orden");
  eq(await texts("#voices .voice .l span"), ["Menos de 1 min de voz", "12,5 min de voz"], "minutos");
  await page.evaluate(() => document.querySelector("#voices").scrollIntoView());
  await shot("07-settings-voices");
  await page.click('#voices [data-act="rename"][data-name="Juan"]');
  await until(() => page.$eval("#dlg", (d) => d.open), "diálogo de nombre");
  await page.fill("#dlg-input", "Juan Pérez");
  await page.keyboard.press("Enter");
  await until(async () => (await texts("#voices .voice b")).includes("Juan Pérez"), "voz renombrada");
  eq(await calls("rename_voice"), [["Juan", "Juan Pérez"]], "rename_voice");
  await page.click('#voices [data-act="forget"][data-name="María"]');
  await until(() => page.$eval("#dlg", (d) => d.open), "confirmación");
  eq(await text("#dlg-ok"), "Olvidar", "botón de confirmar");
  eq(await page.evaluate(() => document.activeElement.id), "dlg-cancel", "el foco empieza en Cancelar");
  await shot("08-forget-confirm");
  await page.click("#dlg-cancel");
  await until(async () => !(await page.$eval("#dlg", (d) => d.open)), "cancelado");
  eq((await calls("forget_voice")).length, 0, "cancelar no borra");
  await page.click('#voices [data-act="forget"][data-name="María"]');
  await page.click("#dlg-ok");
  await until(async () => !(await texts("#voices .voice b")).includes("María"), "voz olvidada");
  eq(await calls("forget_voice"), [["María"]], "forget_voice");
});

await check("Ajustes: General con «Copiar diagnóstico» al final y Anotalo con la versión", async () => {
  await page.click('.nav[data-view="general"]');
  await until(() => page.isVisible("#copy-diag"), "botón de diagnóstico");
  eq((await texts("#view-general .card .set b")).at(-1), "Anotalo", "nombre en Acerca de");
  eq(await text("#version"), "Versión 1.0.0-alpha.2 · desarrollo", "versión");
  const last = await page.$eval("#view-general .wrap", (w) => w.lastElementChild.className);
  eq(last, "diag", "el diagnóstico es lo último");
  const weight = await page.$eval("#copy-diag", (b) => getComputedStyle(b).backgroundColor);
  eq(weight, "rgba(0, 0, 0, 0)", "botón discreto (sin fondo)");
  await page.click("#copy-diag");
  await until(async () => (await text("#toast")).includes("Diagnóstico copiado"), "aviso");
  eq((await calls("copy_diagnostics")).length, 1, "copy_diagnostics");
  await shot("09-settings-general");
});

await check("detalle: «Copiar para Claude» junto a Copiar", async () => {
  await page.click('.nav[data-view="recordings"]');
  await page.click(`.item[data-id="${ACME}"]`);
  await until(async () => (await page.$("#act-claude")) && (await text(".dhead h2")) === "Llamada de Teams", "detalle de Acme");
  eq(await texts(".dhead .actions .btn"), ["Copiar", "Copiar para Claude", "Descargar .md", ""], "botones del detalle");
  eq(await page.getAttribute("#act-claude", "title"), "Copia la transcripción con tu instrucción, para pegarla en Claude", "pista");
  await page.click("#act-claude");
  await until(async () => (await text("#toast")) === "Copiado para Claude", "aviso «Copiado para Claude»");
  eq(await calls("copy_for_claude"), [[ACME]], "copy_for_claude");
  await shot("11-copy-for-claude");
});

await check("General en desarrollo: actualizaciones desactivadas, y la tarjeta de Claude Code", async () => {
  await page.click('.nav[data-view="general"]');
  await until(() => page.isVisible("#claude-connect"), "botón Conectar");
  eq(await text("#upd-text"), "Solo en la versión instalada", "texto de la versión");
  eq(await page.isDisabled("#upd-check"), true, "Buscar actualizaciones desactivado");
  const auto = '[data-setting="auto_update_check"]';
  eq(await page.isDisabled(auto), true, "el interruptor no se puede cambiar en desarrollo");
  eq(await page.$eval(auto, (b) => b.closest(".set").querySelector("b").textContent), "Buscar actualizaciones automáticamente", "nombre del interruptor");
  eq(await page.$eval(auto, (b) => b.closest(".set").querySelector(".l span").textContent), "Al abrir la app y cada 6 horas.", "descripción");
  eq(await page.$eval(auto, (b) => b.closest(".card").querySelector(".ch").textContent), "Actualizaciones", "en su tarjeta");
  const card = await page.$eval("#claude-connect", (b) => b.closest(".card").querySelector(".ch").textContent);
  eq(card, "Claude Code", "tarjeta de Claude Code");
  eq(await page.$eval("#claude-connect", (b) => b.closest(".set").querySelector("b").textContent), "Conectar con Claude Code", "fila de conectar");
  eq(await page.$eval("#claude-connect", (b) => b.closest(".set").querySelector(".l span").textContent),
    "Claude Code podrá leer tus llamadas cuando se lo pidas (solo lectura).", "descripción de conectar");
  eq(await text("#claude-connect"), "Conectar", "botón");
  eq(await page.isVisible("#claude-cmd"), false, "sin orden hasta que falle");
  const inst = await page.evaluate(() => window.__fake.INSTRUCTION);
  eq(await page.getAttribute("#claude-instruction", "placeholder"), inst, "la de por defecto como pista");
  eq(await page.inputValue("#claude-instruction"), inst, "la instrucción guardada");
  eq(await page.getAttribute("#claude-instruction", "maxlength"), "2000", "como mucho 2000 caracteres");
  eq(await page.$eval("#claude-instruction", (t) => t.closest(".set").querySelector("b").textContent), "Instrucción para «Copiar para Claude»", "nombre de la instrucción");
  eq((await texts("#view-general .card .ch")).slice(-2), ["Claude Code", "Acerca de"], "Claude Code antes de Acerca de");
  await page.$eval("#claude-connect", (b) => b.scrollIntoView());
  await shot("12-general-claude-dev");
});

await check("Conectar con Claude Code sin claude: el error y la orden para copiarla", async () => {
  await page.click("#claude-connect");
  await until(() => page.isDisabled("#claude-connect"), "botón ocupado");
  eq(await text("#claude-connect"), "Conectando…", "texto mientras conecta");
  await page.click("#claude-connect", { force: true });  // otro clic no lanza otra conexión
  await until(() => page.isVisible("#claude-cmd"), "la orden");
  eq(await text('[data-err="claude"]'), "No encontré Claude Code (el comando claude) en este PC.", "error");
  eq(await text("#claude-cmd-text"), await page.evaluate(() => window.__fake.COMMAND), "orden");
  eq(await page.isDisabled("#claude-connect"), false, "se puede reintentar");
  eq(await text("#claude-connect"), "Conectar", "texto de vuelta");
  eq((await calls("connect_claude")).length, 1, "una sola conexión");
  await page.click("#claude-cmd-copy");
  await until(async () => (await text("#toast")) === "Copiado al portapapeles", "aviso de copia");
  eq((await calls("copy_claude_command")).length, 1, "copy_claude_command");
  await shot("13-claude-not-found");
});

await check("Conectar con Claude Code: conectado", async () => {
  await page.evaluate(() => { window.__fake.claudeOutcome = { ok: true }; });
  await page.click("#claude-connect");
  await until(async () => (await text("#toast")).startsWith("Conectado."), "aviso de conectado");
  eq(await text("#toast"), "Conectado. En Claude Code puedes pedir: «lee la última llamada con Acme y haz lo que acordamos»", "aviso");
  eq(await page.isVisible("#claude-cmd"), false, "sin la orden");
  eq(await text('[data-err="claude"]'), "", "sin error");
  await shot("14-claude-connected");
});

await check("la instrucción se guarda al salir del campo; vacía, la de por defecto", async () => {
  await page.fill("#claude-instruction", "Resume en tres puntos");
  await page.click("#view-general h1");
  await until(async () => (await calls("set_setting")).some(([k]) => k === "claude_instruction"), "guardada al salir");
  eq((await calls("set_setting")).filter(([k]) => k === "claude_instruction"), [["claude_instruction", "Resume en tres puntos"]], "set_setting");
  await page.fill("#claude-instruction", "");
  await page.click("#view-general h1");
  await until(async () => (await calls("set_setting")).filter(([k]) => k === "claude_instruction").length === 2, "vacía guardada");
  eq((await calls("set_setting")).filter(([k]) => k === "claude_instruction").at(-1), ["claude_instruction", ""], "vacía");
  eq(await page.inputValue("#claude-instruction"), "", "queda vacía (se ve la de por defecto)");
});

await check("tamaño mínimo (940×600): el detalle no se desborda", async () => {
  await page.setViewportSize({ width: 930, height: 590 });
  await page.click('.nav[data-view="recordings"]');
  await page.click('.item[data-id="2026-10-20_1600_Zoom.md"]');
  await until(async () => (await page.$(".ctitle")) && (await text(".ctitle")).startsWith("Seguimiento"), "detalle de Zoom");
  const over = await page.evaluate(() => [...document.querySelectorAll(".dhead, .facts, .list .head, .item")]
    .filter((e) => e.scrollWidth > e.clientWidth + 1).map((e) => e.className));
  eq(over, [], "elementos desbordados");
  await shot("10-detail-min-size");
});

// Con una carpeta lenta, cada línea en directo pedía otra lista sin esperar a la anterior y
// llenaban los hilos de Anotalo (los botones de la ventana esperaban detrás).
await check("carpeta lenta: las líneas en directo piden una lista a la vez, y otra al terminar", async () => {
  const NEW = "2026-11-12_1200_Zoom.md";
  const live = (text) => page.evaluate(([id, text]) =>
    window.onEvent({ type: "line", id, ts: "00:00:20", speaker: "Persona 2", me: false, text }), [LIVE, text]);
  await page.evaluate(() => { window.__fake.listDelay = 2000; window.__fake.listingMax = 0; });
  const before = (await calls("list_recordings")).length;
  await live("Uno.");
  await until(() => page.evaluate(() => window.__fake.listing === 1), "la primera lista en curso");
  // una grabación nueva mientras tanto: la primera lista no la trae
  await page.evaluate((id) => window.__fake.recs.unshift({
    ...window.__fake.recs[0], id, app: "Zoom", time: "12:00", call_title: "Recién guardada", lines: [],
  }), NEW);
  await live("Dos.");
  await sleep(1100);  // su espera de 800 ms ya pasó y la primera sigue en curso
  eq(await page.evaluate(() => window.__fake.listing), 1, "listas en curso");
  await until(async () => (await page.$$(`.item[data-id="${NEW}"]`)).length === 1, "otra carga al terminar trae la nueva", 6000);
  eq(await page.evaluate(() => window.__fake.listingMax), 1, "como mucho una lista a la vez");
  eq((await calls("list_recordings")).length - before, 2, "la primera y una más");
  // vuelta a como estaba
  await page.evaluate(() => { window.__fake.listDelay = 0; window.__fake.recs.shift(); window.onEvent({ type: "recordings" }); });
  await until(async () => (await page.$$(".item")).length === 7, "las 7 de antes");
});

await check("barras de desplazamiento con el estilo de la app en toda la ventana", async () => {
  const st = await page.evaluate(() => {
    const rules = [...document.styleSheets].flatMap((s) => [...s.cssRules]);
    const thumb = rules.find((r) => r.selectorText === "::-webkit-scrollbar-thumb");
    // con scrollbar-color o scrollbar-width en un elemento, Chromium ignora ::-webkit-scrollbar
    const std = [...document.querySelectorAll("*")].filter((e) => {
      const cs = getComputedStyle(e);
      return cs.scrollbarColor !== "auto" || cs.scrollbarWidth !== "auto";
    }).length;
    return { thumb: thumb ? thumb.style.backgroundColor : null, std };
  });
  if (!st.thumb) throw new Error("sin regla ::-webkit-scrollbar-thumb");
  eq(st.std, 0, "elementos con scrollbar-color/width");
  await page.evaluate(() => { const b = document.querySelector(".items"); if (b) b.scrollTop = 40; });
  await shot("16-scrollbar");
});

await check("sin errores en la consola, peticiones fallidas ni órdenes desconocidas", async () => {
  const fake = await page.evaluate(() => ({
    unknown: window.__fake.unknown,
    logs: window.__fake.calls.filter(([m, a]) => m === "log_js" && a[0] !== "ready").map(([, a]) => a[0]),
  }));
  eq(fake.unknown, [], "órdenes desconocidas");
  eq(fake.logs, [], "errores enviados con log_js");
  eq(problems, [], "consola y red");
});

// --- la versión instalada (con actualizador) ------------------------------------
await page.setViewportSize({ width: 1110, height: 710 });
await page.goto(`${ORIGIN}index.html?installed`);
const upd = (u) => page.evaluate((u) => window.onEvent({ type: "update", ...window.__fake.update, ...u }), u);

await check("instalada: buscar, disponible, actualizar, descargando, reintentar", async () => {
  await until(async () => (await page.$$(".item")).length > 0, "la lista");
  await page.click('.nav[data-view="general"]');
  await until(async () => (await text("#upd-text")) === "Tienes la versión 1.0.0-alpha.2.", "versión actual");
  eq(await page.isDisabled("#upd-check"), false, "Buscar actualizaciones activo");
  eq(await page.isDisabled('[data-setting="auto_update_check"]'), false, "interruptor activo");
  eq(await page.isVisible("#upd-install"), false, "sin Actualizar");
  eq(await text("#version"), "Versión 1.0.0-alpha.2", "sin «desarrollo»");
  await page.click("#upd-check");
  await until(async () => (await calls("check_updates")).length === 1, "check_updates");
  await upd({ state: "checking" });
  await until(async () => (await text("#upd-text")) === "Buscando actualizaciones…", "buscando");
  eq(await page.isDisabled("#upd-check"), true, "no se busca dos veces");
  await upd({ state: "available", version: "1.0.0-alpha.3" });
  await until(async () => (await text("#upd-text")) === "Versión 1.0.0-alpha.3 disponible.", "disponible");
  eq(await text("#upd-install"), "Actualizar", "botón Actualizar");
  eq(await page.isVisible("#upd-note"), true, "aviso en la barra lateral");
  eq(await text("#upd-note-title"), "Versión 1.0.0-alpha.3 disponible", "título del aviso");
  await shot("15-update-available");
  await page.click("#upd-install");
  await until(async () => (await calls("install_update")).length === 1, "install_update");
  await upd({ state: "downloading", version: "1.0.0-alpha.3", progress: 0.42 });
  await until(async () => (await text("#upd-text")) === "Descargando la versión 1.0.0-alpha.3… 42 %", "descargando");
  eq(await page.isVisible("#upd-install"), false, "sin Actualizar mientras descarga");
  await upd({ state: "waiting", version: "1.0.0-alpha.3" });
  await until(async () => (await text("#upd-text")) === "La versión 1.0.0-alpha.3 se instalará al terminar la grabación.", "esperando");
  await upd({ state: "error", version: "1.0.0-alpha.3", error: "La instalación no se completó. Vuelve a intentarlo." });
  await until(async () => (await text("#upd-install")) === "Reintentar", "Reintentar");
  eq(await text('[data-err="update"]'), "La instalación no se completó. Vuelve a intentarlo.", "error");
  await page.click("#upd-note-btn");
  await until(async () => (await calls("install_update")).length === 2, "reintentar desde la barra lateral");
  await page.click('[data-setting="auto_update_check"]');
  await until(async () => (await calls("set_setting")).some(([k, v]) => k === "auto_update_check" && v === false), "apagar la búsqueda automática");
  eq(await page.getAttribute('[data-setting="auto_update_check"]', "aria-checked"), "false", "interruptor apagado");
});

await check("instalada: sin errores ni órdenes desconocidas", async () => {
  const fake = await page.evaluate(() => ({
    unknown: window.__fake.unknown,
    logs: window.__fake.calls.filter(([m, a]) => m === "log_js" && a[0] !== "ready").map(([, a]) => a[0]),
  }));
  eq(fake.unknown, [], "órdenes desconocidas");
  eq(fake.logs, [], "errores enviados con log_js");
  eq(problems, [], "consola y red");
});

await browser.close();
for (const [ok, name, why] of results) console.log(`${ok ? "OK   " : "FALLA"} ${name}${why ? `\n      ${why}` : ""}`);
console.log(`\nCapturas (${shots.length}) en ${OUT}`);
const failed = results.filter(([ok]) => !ok).length;
console.log(failed ? `${failed} de ${results.length} comprobaciones fallan` : `${results.length} comprobaciones, todas bien`);
process.exit(failed ? 1 : 0);
