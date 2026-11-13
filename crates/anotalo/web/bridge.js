// Puente entre la página y Anotalo (Rust). Mantiene `window.pywebview.api.<orden>(…)` de la app de
// Python: cada llamada va por `window.ipc` como {id, method, args} y Rust responde con
// `window.__anotaloReply(id, resultado)`. Los avisos siguen llegando por `window.onEvent(…)`.
// Se carga antes que el resto de scripts de la página (ver crates/anotalo/src/ui_bridge.rs).
(function () {
  "use strict";
  let next = 1;
  const pending = new Map();

  // Nunca rechaza: si no hay con quién hablar, responde como una orden fallida.
  function call(method, args) {
    return new Promise((resolve) => {
      const id = next++;
      pending.set(id, resolve);
      try {
        window.ipc.postMessage(JSON.stringify({ id, method, args }));
      } catch (e) {
        pending.delete(id);
        console.error("Sin conexión con Anotalo", e);
        resolve({ ok: false, error: "Sin conexión con Anotalo" });
      }
    });
  }

  window.__anotaloReply = (id, result) => {
    const resolve = pending.get(id);
    if (resolve) {
      pending.delete(id);
      resolve(result);
    }
  };

  // Cualquier nombre es una orden, salvo `then` (si no, `await api` la tomaría por una promesa)
  // y los símbolos.
  const api = new Proxy({}, {
    get: (_, name) =>
      typeof name === "string" && name !== "then" ? (...args) => call(name, args) : undefined,
  });
  window.pywebview = { api };

  function ready() {
    call("log_js", ["ready"]);
    window.dispatchEvent(new Event("pywebviewready"));
  }
  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", ready);
  } else {
    ready();
  }
})();
