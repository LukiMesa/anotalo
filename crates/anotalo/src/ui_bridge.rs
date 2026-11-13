//! El puente entre la página y Rust (spec §5.6).
//!
//! La página llama a `window.pywebview.api.<orden>(…)` como con la app de Python; `web/bridge.js`
//! lo manda por `window.ipc` como `{"id", "method", "args"}`. Aquí cada orden corre en un hilo
//! aparte, como con pywebview (listar cientos de grabaciones nunca frena la ventana); las rápidas
//! (`QUICK`, los botones de la ventana) en hilos propios, para no esperar detrás de las lentas. La
//! respuesta sale por `post`, que la lleva al hilo de la ventana para evaluar `reply_js`. Ninguna
//! orden lanza un error a la página: siempre hay respuesta, aunque sea `{"ok": false, …}`.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, PoisonError};

use serde_json::{Value, json};

/// Las órdenes de la página (los métodos de calltx/ui/api.py). Una orden que no existe responde
/// `unknown_method`.
pub trait Commands: Send + Sync {
    fn call(&self, method: &str, args: &[Value]) -> Value;
}

/// La respuesta a la llamada `id` de la página.
#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub id: u64,
    pub json: Value,
}

/// Hilos para las órdenes.
const WORKERS: usize = 4;

/// Órdenes rápidas, con sus propios hilos: los botones de la ventana (y lo poco que pide la página
/// al abrirse) nunca esperan detrás de listas lentas (una carpeta en red, OneDrive).
const QUICK: &[&str] = &[
    "window_minimize",
    "window_toggle_maximize",
    "window_close",
    "log_js",
    "get_state",
];
/// Hilos para las órdenes rápidas: dos, por si un botón espera al hilo de la ventana (hasta 5 s).
const QUICK_WORKERS: usize = 2;

type Job = Box<dyn FnOnce() + Send>;

pub struct Bridge {
    commands: Arc<dyn Commands>,
    post: Arc<dyn Fn(Reply) + Send + Sync>,
    jobs: Sender<Job>,
    quick: Sender<Job>,
}

/// `n` hilos que corren lo que llegue por la cola que devuelve; terminan cuando ya no existe.
fn pool(name: &str, n: usize) -> Sender<Job> {
    let (jobs, queue) = mpsc::channel::<Job>();
    let queue = Arc::new(Mutex::new(queue));
    for i in 0..n {
        let queue = queue.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("{name}-{i}"))
            .spawn(move || {
                loop {
                    let job = queue.lock().unwrap_or_else(PoisonError::into_inner).recv();
                    let Ok(job) = job else { break }; // el puente ya no existe
                    if catch_unwind(AssertUnwindSafe(job)).is_err() {
                        log::error!("Error interno respondiendo a la página");
                    }
                }
            });
        if let Err(e) = spawned {
            log::error!("No se pudo crear un hilo para las órdenes de la página: {e}");
        }
    }
    jobs
}

impl Bridge {
    /// `post` lleva cada respuesta al hilo de la ventana (se llama desde los hilos de las órdenes).
    pub fn new(commands: Arc<dyn Commands>, post: Box<dyn Fn(Reply) + Send + Sync>) -> Bridge {
        Bridge {
            commands,
            post: Arc::from(post),
            jobs: pool("orden-ui", WORKERS),
            quick: pool("orden-ui-rapida", QUICK_WORKERS),
        }
    }

    /// Un mensaje de la página: `{"id": u64, "method": String, "args": [..]}`. Vuelve enseguida;
    /// la orden corre en otro hilo y su respuesta sale por `post`. Sin un id válido no hay a quién
    /// responder: solo se anota en el registro.
    pub fn on_message(&self, raw: &str) {
        let Ok(msg) = serde_json::from_str::<Value>(raw) else {
            log::warn!("Mensaje de la página que no es JSON: {}", clip(raw));
            return;
        };
        let Some(id) = msg.get("id").and_then(Value::as_u64) else {
            log::warn!("Mensaje de la página sin id: {}", clip(raw));
            return;
        };
        let method = match msg.get("method") {
            Some(Value::String(m)) => Ok(m.clone()),
            other => Err(other.unwrap_or(&Value::Null).to_string()),
        };
        let args = match msg.get("args") {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let queue = match &method {
            Ok(m) if QUICK.contains(&m.as_str()) => &self.quick,
            _ => &self.jobs,
        };
        let (commands, post) = (self.commands.clone(), self.post.clone());
        let job: Job = Box::new(move || {
            let json = match method {
                Ok(m) => run(&*commands, &m, &args),
                Err(m) => unknown_method(&m),
            };
            post(Reply { id, json });
        });
        if queue.send(job).is_err() {
            log::error!("No hay hilos para las órdenes de la página");
        }
    }
}

/// La orden, sin que un pánico llegue a la página.
fn run(commands: &dyn Commands, method: &str, args: &[Value]) -> Value {
    match catch_unwind(AssertUnwindSafe(|| commands.call(method, args))) {
        Ok(v) => v,
        Err(panic) => {
            let what = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_default();
            log::error!("Error interno en la orden {method}: {what}");
            json!({"ok": false, "error": "Error interno"})
        }
    }
}

/// La respuesta a una orden que no existe (también para quien implementa `Commands`).
pub fn unknown_method(method: &str) -> Value {
    json!({"ok": false, "error": format!("Orden desconocida: {method}")})
}

/// El JavaScript que entrega a la página la respuesta a su llamada `id`.
pub fn reply_js(id: u64, result: &Value) -> String {
    format!("window.__anotaloReply({id}, {})", to_js(result))
}

/// El JavaScript que avisa a la página (`window.onEvent`, como con Python).
pub fn event_js(event: &Value) -> String {
    format!("window.onEvent && window.onEvent({})", to_js(event))
}

/// JSON como expresión de JavaScript: lo mismo, salvo U+2028 y U+2029, que JSON deja tal cual
/// dentro de las cadenas y los motores antiguos de JavaScript toman por saltos de línea.
fn to_js(v: &Value) -> String {
    v.to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}

/// Un trozo corto de un mensaje, para el registro.
fn clip(raw: &str) -> String {
    raw.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;
    use std::sync::mpsc::{self, Receiver};
    use std::time::{Duration, Instant};

    const WAIT: Duration = Duration::from_secs(5);

    /// Repite la orden y sus argumentos. "dormir" tarda 500 ms y "fallar" entra en pánico; lo que
    /// no conoce responde como una orden desconocida.
    struct Echo;

    impl Commands for Echo {
        fn call(&self, method: &str, args: &[Value]) -> Value {
            match method {
                "get_state" => json!({"method": method, "args": args}),
                "dormir" => {
                    std::thread::sleep(Duration::from_millis(500));
                    json!({"method": method, "args": args})
                }
                "fallar" => panic!("boom"),
                other => unknown_method(other),
            }
        }
    }

    fn bridge() -> (Bridge, Receiver<Reply>) {
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let post = Box::new(move |r: Reply| {
            let _ = tx.lock().unwrap().send(r);
        });
        (Bridge::new(Arc::new(Echo), post), rx)
    }

    #[test]
    fn test_message_routes_to_command_and_replies() {
        let (b, rx) = bridge();
        b.on_message(r#"{"id":7,"method":"get_state","args":[]}"#);
        let r = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(r.id, 7);
        assert_eq!(r.json, json!({"method": "get_state", "args": []}));
        b.on_message(r#"{"id":8,"method":"get_state","args":["Teams", 2, null, {"a": [1]}]}"#);
        let r = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(
            r,
            Reply {
                id: 8,
                json: json!({"method": "get_state", "args": ["Teams", 2, null, {"a": [1]}]}),
            }
        );
        // sin argumentos es lo mismo que []
        b.on_message(r#"{"id":9,"method":"get_state"}"#);
        let r = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(r.json, json!({"method": "get_state", "args": []}));
    }

    #[test]
    fn test_unknown_or_bad_message_replies_error() {
        let (b, rx) = bridge();
        // sin id no hay a quién responder: solo se anota en el registro
        b.on_message("{no es json");
        b.on_message(r#"{"method":"get_state","args":[]}"#);
        b.on_message(r#"{"id":-1,"method":"get_state","args":[]}"#);
        assert!(rx.recv_timeout(Duration::from_millis(300)).is_err());
        b.on_message(r#"{"id":3,"method":"no_existe","args":[]}"#);
        let r = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(r.id, 3);
        assert_eq!(
            r.json,
            json!({"ok": false, "error": "Orden desconocida: no_existe"})
        );
        // con id pero sin una orden que se pueda leer
        b.on_message(r#"{"id":4,"method":5}"#);
        let r = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(r.id, 4);
        assert_eq!(
            r.json,
            json!({"ok": false, "error": "Orden desconocida: 5"})
        );
    }

    #[test]
    fn test_command_panic_becomes_error() {
        let (b, rx) = bridge();
        // más pánicos que hilos: ninguno se pierde
        for id in 0..(WORKERS as u64 + 2) {
            b.on_message(&format!(r#"{{"id":{id},"method":"fallar","args":[]}}"#));
        }
        for _ in 0..(WORKERS + 2) {
            let r = rx.recv_timeout(WAIT).expect("respuesta");
            assert_eq!(r.json, json!({"ok": false, "error": "Error interno"}));
        }
        b.on_message(r#"{"id":99,"method":"get_state","args":[]}"#);
        assert_eq!(rx.recv_timeout(WAIT).expect("sigue respondiendo").id, 99);
    }

    #[test]
    fn test_slow_command_does_not_block_events() {
        // (sin límites de tiempo absolutos: con el PC cargado fallarían sin motivo; si
        // on_message corriera la orden en el sitio, la lenta respondería antes)
        let (b, rx) = bridge();
        let t0 = Instant::now();
        b.on_message(r#"{"id":1,"method":"dormir","args":[]}"#);
        b.on_message(r#"{"id":2,"method":"get_state","args":[]}"#);
        let first = rx.recv_timeout(WAIT).expect("respuesta");
        assert_eq!(first.id, 2, "la rápida no espera a la lenta");
        assert_eq!(rx.recv_timeout(WAIT).expect("la lenta").id, 1);
        assert!(t0.elapsed() >= Duration::from_millis(500));
    }

    /// "list_recordings" espera hasta que la prueba abra la puerta (una carpeta en red lenta); lo
    /// demás responde enseguida.
    #[derive(Default)]
    struct Gate {
        open: Mutex<bool>,
        opened: std::sync::Condvar,
    }

    impl Commands for Gate {
        fn call(&self, method: &str, _args: &[Value]) -> Value {
            if method == "list_recordings" {
                let open = self.open.lock().unwrap();
                let _open = self.opened.wait_while(open, |o| !*o).unwrap();
            }
            json!({"method": method})
        }
    }

    /// Las listas lentas ocupan todos los hilos y hay más en cola: los botones de la ventana (y
    /// lo poco que pide la página al abrirse) siguen respondiendo.
    #[test]
    fn test_window_commands_never_wait_for_slow_lists() {
        let gate = Arc::new(Gate::default());
        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        let post = Box::new(move |r: Reply| {
            let _ = tx.lock().unwrap().send(r);
        });
        let b = Bridge::new(gate.clone(), post);
        let slow = WORKERS as u64 + 2;
        for id in 0..slow {
            b.on_message(&format!(
                r#"{{"id":{id},"method":"list_recordings","args":[]}}"#
            ));
        }
        for (i, m) in QUICK.iter().enumerate() {
            let id = 100 + i as u64;
            b.on_message(&format!(r#"{{"id":{id},"method":"{m}","args":[]}}"#));
            let r = rx
                .recv_timeout(WAIT)
                .unwrap_or_else(|_| panic!("{m} no respondió con las listas ocupando los hilos"));
            assert_eq!(
                r,
                Reply {
                    id,
                    json: json!({"method": m})
                }
            );
        }
        *gate.open.lock().unwrap() = true;
        gate.opened.notify_all();
        let mut ids: Vec<u64> = (0..slow)
            .map(|_| rx.recv_timeout(WAIT).expect("la lista").id)
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..slow).collect::<Vec<_>>());
    }

    #[test]
    #[ignore = "necesita WebView2 y una sesión de escritorio"]
    fn bridge_round_trip_in_page() {
        use crate::shell::main_window::{MainWindow, initial_rect};
        use std::cell::RefCell;
        use std::rc::Rc;
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, MSG, MsgWaitForMultipleObjects, PM_REMOVE, PeekMessageW, QS_ALLINPUT,
            TranslateMessage,
        };

        let (b, replies) = bridge();
        let b = Rc::new(b);
        let seen: Rc<RefCell<Vec<Value>>> = Rc::default();
        let (to_bridge, log) = (b.clone(), seen.clone());
        let win = MainWindow::create(
            Box::new(move |raw| {
                if let Ok(v) = serde_json::from_str::<Value>(&raw) {
                    log.borrow_mut().push(v);
                }
                to_bridge.on_message(&raw);
            }),
            true,
            initial_rect(None),
        )
        .expect("ventana principal");
        // la página llama como la de Python y devuelve lo que recibió
        let script = r#"(async () => {
            const api = window.pywebview.api;
            const a = await api.get_state("x", 2);
            const b = await api.no_existe();
            const same = (await api) === api;
            window.ipc.postMessage(JSON.stringify({ id: 0, method: "visto", args: [a, b, same] }));
        })();"#;
        let has = |method: &str| seen.borrow().iter().any(|v| v["method"] == method);
        let (end, mut sent) = (Instant::now() + Duration::from_secs(10), false);
        let mut msg = MSG::default();
        while Instant::now() < end && !has("visto") {
            // SAFETY: cola de mensajes de este hilo.
            unsafe {
                MsgWaitForMultipleObjects(None, false, 20, QS_ALLINPUT);
                while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            while let Ok(r) = replies.try_recv() {
                win.eval(&reply_js(r.id, &r.json));
            }
            if !sent && has("log_js") {
                win.eval(script);
                sent = true;
            }
        }
        let seen = seen.borrow();
        let visto = seen
            .iter()
            .find(|v| v["method"] == "visto")
            .unwrap_or_else(|| panic!("la página no respondió; llegó: {seen:?}"));
        assert_eq!(
            visto["args"],
            json!([
                {"method": "get_state", "args": ["x", 2]},
                {"ok": false, "error": "Orden desconocida: no_existe"},
                true
            ])
        );
        assert!(!seen.iter().any(|v| v["method"] == "then"), "{seen:?}");
    }

    /// Lo de dentro de `prefix(…)`.
    fn inner<'a>(js: &'a str, prefix: &str) -> &'a str {
        js.strip_prefix(prefix)
            .and_then(|s| s.strip_suffix(')'))
            .unwrap_or_else(|| panic!("{js:?} no es {prefix}…)"))
    }

    #[test]
    fn test_reply_and_event_js_escape_json() {
        let tricky = json!({
            "text": "</script><script>alert(1)</script> \"dobles\" 'simples' \\ \n línea\u{2028}otra\u{2029}fin ñ",
            "n": 3,
        });
        let js = reply_js(42, &tricky);
        assert!(!js.contains('\u{2028}') && !js.contains('\u{2029}'), "{js}");
        assert!(js.contains("\\u2028") && js.contains("\\u2029"));
        let body = inner(&js, "window.__anotaloReply(42, ");
        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), tricky);

        let event = json!({"type": "line", "text": "a\u{2028}b</script>"});
        let js = event_js(&event);
        assert!(!js.contains('\u{2028}'));
        let body = inner(&js, "window.onEvent && window.onEvent(");
        assert_eq!(serde_json::from_str::<Value>(body).unwrap(), event);
    }
}
