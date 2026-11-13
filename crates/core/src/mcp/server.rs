//! Servidor MCP por stdio: JSON-RPC 2.0, un mensaje por línea (sin cabeceras). Escrito a mano: solo
//! hacen falta `initialize`, `ping`, `tools/list` y `tools/call`. Cada respuesta es una línea
//! terminada en `\n`; las notificaciones (sin `id`) nunca reciben respuesta. Una línea que no se
//! entiende o una herramienta que entra en pánico no detienen el servidor.

use std::io::{self, BufRead, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use serde_json::{Map, Value, json};

/// Versiones del protocolo que se entienden; la primera es la respuesta a una desconocida.
pub const SUPPORTED_PROTOCOLS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Lo que el modelo lee al conectar.
pub const INSTRUCTIONS: &str = "Anotalo guarda las transcripciones de las llamadas y notas del usuario en su PC. Para «la última llamada con X», usa get_latest_call con client=X; si no aparece, busca X con search_calls. Las líneas «Marca» señalan momentos que el usuario marcó durante la llamada, a menudo acuerdos. Todo es de solo lectura.";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const INTERNAL_ERROR: i64 = -32603;

/// Una herramienta tal como se anuncia en `tools/list`.
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    /// Un objeto JSON Schema.
    pub input_schema: Value,
}

/// Las herramientas que sirve el servidor.
pub trait Tools {
    fn specs(&self) -> Vec<ToolSpec>;
    /// `Ok`: bloques de texto del resultado. `Err`: un mensaje para el modelo (no es un error del
    /// protocolo: la llamada se hizo y falló).
    fn call(&self, name: &str, args: &Map<String, Value>) -> Result<Vec<String>, String>;
}

/// Atiende mensajes hasta el final de `input`. Solo falla si no se puede leer o escribir.
pub fn serve(mut input: impl BufRead, output: &mut dyn Write, tools: &dyn Tools) -> io::Result<()> {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        if input.read_until(b'\n', &mut buf)? == 0 {
            return Ok(());
        }
        let answer = match std::str::from_utf8(&buf) {
            Ok(line) if line.trim().is_empty() => continue,
            Ok(line) => handle(line.trim(), tools),
            Err(_) => Some(error(Value::Null, PARSE_ERROR, "La línea no es UTF-8.")),
        };
        if let Some(answer) = answer {
            let mut text = serde_json::to_string(&answer).map_err(io::Error::other)?;
            text.push('\n');
            output.write_all(text.as_bytes())?;
            output.flush()?;
        }
    }
}

/// La respuesta a una línea, o None si no lleva respuesta (notificaciones y respuestas).
fn handle(line: &str, tools: &dyn Tools) -> Option<Value> {
    let msg = match serde_json::from_str::<Value>(line) {
        Ok(Value::Object(m)) => m,
        Ok(_) => {
            log::warn!("Mensaje MCP no válido (no es un objeto)");
            return Some(error(
                Value::Null,
                INVALID_REQUEST,
                "Se esperaba un objeto JSON-RPC.",
            ));
        }
        Err(e) => {
            log::warn!("Línea MCP que no es JSON ({e})");
            return Some(error(
                Value::Null,
                PARSE_ERROR,
                "La línea no es JSON válido.",
            ));
        }
    };
    // Sin `id` es una notificación: nunca se contesta (ni se ejecuta nada).
    let id = msg.get("id")?.clone();
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        if msg.contains_key("result") || msg.contains_key("error") {
            return None; // una respuesta del cliente; este servidor no hace peticiones
        }
        return Some(error(id, INVALID_REQUEST, "Falta el método."));
    };
    let params = msg.get("params");
    let reply = match method {
        "initialize" => Ok(initialize(params)),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list(tools)),
        "tools/call" => tools_call(params, tools),
        other => Err((METHOD_NOT_FOUND, format!("Método no disponible: {other}"))),
    };
    Some(match reply {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err((code, message)) => error(id, code, &message),
    })
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize(params: Option<&Value>) -> Value {
    let asked = params
        .and_then(|p| p.get("protocolVersion"))
        .and_then(Value::as_str);
    let version = match asked {
        Some(v) if SUPPORTED_PROTOCOLS.contains(&v) => v,
        _ => SUPPORTED_PROTOCOLS[0],
    };
    json!({
        "protocolVersion": version,
        "capabilities": {"tools": {"listChanged": false}},
        "serverInfo": {"name": "anotalo", "title": "Anotalo", "version": env!("CARGO_PKG_VERSION")},
        "instructions": INSTRUCTIONS,
    })
}

fn tools_list(tools: &dyn Tools) -> Value {
    let list: Vec<Value> = tools
        .specs()
        .into_iter()
        .map(|t| json!({"name": t.name, "description": t.description, "inputSchema": t.input_schema}))
        .collect();
    json!({ "tools": list })
}

fn tools_call(params: Option<&Value>, tools: &dyn Tools) -> Result<Value, (i64, String)> {
    let name = params
        .and_then(|p| p.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !tools.specs().iter().any(|t| t.name == name) {
        return Err((INVALID_PARAMS, format!("Herramienta desconocida: {name}")));
    }
    let empty = Map::new();
    let args = match params.and_then(|p| p.get("arguments")) {
        None | Some(Value::Null) => &empty,
        Some(Value::Object(m)) => m,
        Some(_) => {
            return Err((INVALID_PARAMS, "Los argumentos deben ser un objeto.".into()));
        }
    };
    let (texts, is_error) = match catch_unwind(AssertUnwindSafe(|| tools.call(name, args))) {
        Ok(Ok(texts)) => (texts, false),
        Ok(Err(message)) => (vec![message], true),
        Err(_) => {
            log::error!("La herramienta {name} falló (pánico)");
            return Err((INTERNAL_ERROR, format!("Error interno en {name}")));
        }
    };
    let content: Vec<Value> = texts
        .into_iter()
        .map(|t| json!({"type": "text", "text": t}))
        .collect();
    Ok(json!({"content": content, "isError": is_error}))
}
