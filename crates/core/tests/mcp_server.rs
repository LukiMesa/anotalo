//! El servidor MCP (JSON-RPC 2.0, un mensaje por línea) con una herramienta de prueba.
use anotalo_core::mcp::server::{INSTRUCTIONS, ToolSpec, Tools, serve};
use serde_json::{Map, Value, json};

/// Una herramienta `echo`: devuelve `t`, falla con "mal" y entra en pánico con "pánico".
struct FakeTools;

impl Tools for FakeTools {
    fn specs(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "echo",
            description: "Repite el texto t.",
            input_schema: json!({"type": "object", "properties": {"t": {"type": "string"}}}),
        }]
    }

    fn call(&self, name: &str, args: &Map<String, Value>) -> Result<Vec<String>, String> {
        assert_eq!(name, "echo");
        let t = args.get("t").and_then(Value::as_str).unwrap_or_default();
        match t {
            "mal" => Err("mal".into()),
            "pánico" => panic!("pánico en la herramienta"),
            _ => Ok(vec![t.to_string()]),
        }
    }
}

fn run(lines: &[&str]) -> Vec<Value> {
    let mut out = Vec::new();
    serve(lines.join("\n").as_bytes(), &mut out, &FakeTools).unwrap();
    String::from_utf8(out)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

const INIT: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"claude-code","version":"2"}}}"#;

#[test]
fn test_initialize_echoes_a_known_version() {
    let r = run(&[INIT]);
    assert_eq!(r[0]["id"], 1);
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(r[0]["result"]["serverInfo"]["name"], "anotalo");
    assert_eq!(
        r[0]["result"]["capabilities"],
        json!({"tools": {"listChanged": false}})
    );
    assert_eq!(r[0]["result"]["instructions"], INSTRUCTIONS);
}

#[test]
fn test_initialize_with_an_unknown_version_answers_the_latest() {
    let r = run(&[&INIT.replace("2025-06-18", "1999-01-01")]);
    assert_eq!(r[0]["result"]["protocolVersion"], "2025-11-25");
}

#[test]
fn test_notifications_get_no_answer() {
    let r = run(&[
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"echo"}}"#,
        r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#,
    ]);
    assert_eq!(r, [json!({"jsonrpc":"2.0","id":2,"result":{}})]);
}

#[test]
fn test_tools_list_and_call() {
    let r = run(&[
        r#"{"jsonrpc":"2.0","id":"a","method":"tools/list"}"#,
        r#"{"jsonrpc":"2.0","id":"b","method":"tools/call","params":{"name":"echo","arguments":{"t":"hola\ny"}}}"#,
    ]);
    assert_eq!(r[0]["result"]["tools"][0]["name"], "echo");
    assert!(r[0]["result"]["tools"][0]["inputSchema"].is_object());
    assert_eq!(
        r[1]["result"],
        json!({"content":[{"type":"text","text":"hola\ny"}],"isError":false})
    );
}

#[test]
fn test_tool_failure_is_a_result_not_a_protocol_error() {
    let r = run(&[
        r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"t":"mal"}}}"#,
    ]);
    assert_eq!(r[0]["result"]["isError"], true);
    assert_eq!(r[0]["result"]["content"][0]["text"], "mal");
}

#[test]
fn test_unknown_tool_method_and_bad_arguments() {
    let r = run(&[
        r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"delete_call","arguments":{}}}"#,
        r#"{"jsonrpc":"2.0","id":5,"method":"resources/list"}"#,
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"echo","arguments":[1]}}"#,
    ]);
    assert_eq!(
        [
            &r[0]["error"]["code"],
            &r[1]["error"]["code"],
            &r[2]["error"]["code"]
        ],
        [-32602, -32601, -32602]
    );
}

#[test]
fn test_bad_lines_do_not_stop_the_server() {
    let r = run(&[
        "{no es json",
        "[]",
        "",
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"echo","arguments":{"t":"pánico"}}}"#,
        r#"{"jsonrpc":"2.0","id":8,"method":"ping"}"#,
    ]);
    assert_eq!(r.len(), 4);
    assert_eq!(
        (&r[0]["error"]["code"], &r[0]["id"]),
        (&json!(-32700), &Value::Null)
    );
    assert_eq!(r[1]["error"]["code"], -32600);
    assert_eq!(r[2]["error"]["code"], -32603);
    assert_eq!(r[3]["id"], 8);
}

#[test]
fn test_eof_ends_cleanly() {
    let mut out = Vec::new();
    serve(&b""[..], &mut out, &FakeTools).unwrap();
    assert!(out.is_empty());
}
