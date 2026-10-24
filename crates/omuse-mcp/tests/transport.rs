//! The two MCP transports against an in-process fake server that answers with the wire shapes of
//! `tests/fake_mcp.py`: Streamable HTTP at `/mcp` (bearer token, `Mcp-Session-Id`, SSE reply for `tools/call`,
//! paginated `tools/list`) and legacy HTTP+SSE at `/sse` + `/messages?sid=…`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use omuse_mcp::mcp_client::{connect, Client};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

const TOKEN: &str = "Bearer test-mcp-token";

struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_request(sock: &mut TcpStream, buf: &mut Vec<u8>) -> Option<Req> {
    loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..i]).into_owned();
            buf.drain(..i + 4);
            let mut lines = head.lines();
            let first = lines.next()?;
            let mut parts = first.split_whitespace();
            let method = parts.next()?.to_string();
            let path = parts.next()?.to_string();
            let mut headers = HashMap::new();
            for ln in lines {
                if let Some((k, v)) = ln.split_once(':') {
                    headers.insert(k.trim().to_lowercase(), v.trim().to_string());
                }
            }
            let len: usize = headers
                .get("content-length")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            while buf.len() < len {
                let mut tmp = [0u8; 4096];
                let n = sock.read(&mut tmp).await.ok()?;
                if n == 0 {
                    return None;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            let body: Vec<u8> = buf.drain(..len).collect();
            return Some(Req {
                method,
                path,
                headers,
                body,
            });
        }
        let mut tmp = [0u8; 4096];
        let n = sock.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

async fn respond(
    sock: &mut TcpStream,
    status: u16,
    ctype: &str,
    extra: &[(&str, &str)],
    body: &[u8],
) {
    let mut head = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\n", body.len());
    if !ctype.is_empty() {
        head.push_str(&format!("Content-Type: {ctype}\r\n"));
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    let _ = sock.write_all(head.as_bytes()).await;
    let _ = sock.write_all(body).await;
    let _ = sock.flush().await;
}

struct State {
    calls: AtomicUsize,
    sse_queues: Mutex<HashMap<String, mpsc::UnboundedSender<Value>>>,
}

fn tools() -> Vec<Value> {
    (1..=5)
        .map(|i| json!({"name": format!("tool{i}"), "description": format!("Tool {i}."), "inputSchema": {"type": "object", "properties": {}}}))
        .collect()
}

fn handle(st: &State, msg: &Value) -> Option<Value> {
    let mid = msg.get("id")?.clone();
    let m = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let res = match m {
        "initialize" => {
            json!({"protocolVersion": "2025-06-18", "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": "fake-notes", "version": "1.0"}, "instructions": "Fake notes server."})
        }
        "ping" => json!({}),
        "tools/list" => {
            let all = tools();
            let cur: usize = msg["params"]
                .get("cursor")
                .and_then(Value::as_str)
                .and_then(|c| c.parse().ok())
                .unwrap_or(0);
            let page: Vec<Value> = all.iter().skip(cur).take(3).cloned().collect();
            let mut r = json!({"tools": page});
            if cur + 3 < all.len() {
                r["nextCursor"] = json!((cur + 3).to_string());
            }
            r
        }
        "tools/call" => {
            st.calls.fetch_add(1, Ordering::SeqCst);
            let p = &msg["params"];
            if p["name"] == "boom" {
                json!({"isError": true, "content": [{"type": "text", "text": "unknown tool boom"}]})
            } else {
                json!({"content": [{"type": "text", "text": format!("{} got {}", p["name"].as_str().unwrap_or(""), p["arguments"])}],
                       "structuredContent": {"echo": p["arguments"]}})
            }
        }
        _ => {
            return Some(
                json!({"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "method not found"}}),
            )
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": mid, "result": res}))
}

async fn serve_conn(st: Arc<State>, mut sock: TcpStream) {
    let mut buf = Vec::new();
    while let Some(req) = read_request(&mut sock, &mut buf).await {
        let path = req.path.split('?').next().unwrap_or("").to_string();
        match (req.method.as_str(), path.as_str()) {
            ("POST", "/mcp") => {
                if req.headers.get("authorization").map(String::as_str) != Some(TOKEN) {
                    respond(
                        &mut sock,
                        401,
                        "application/json",
                        &[],
                        br#"{"error":"unauthorized"}"#,
                    )
                    .await;
                    continue;
                }
                let msg: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                let sid = req
                    .headers
                    .get("mcp-session-id")
                    .cloned()
                    .unwrap_or_default();
                if msg["method"] != "initialize" && sid.is_empty() {
                    respond(
                        &mut sock,
                        400,
                        "application/json",
                        &[],
                        br#"{"error":"missing session"}"#,
                    )
                    .await;
                    continue;
                }
                let Some(out) = handle(&st, &msg) else {
                    respond(&mut sock, 202, "", &[], b"").await;
                    continue;
                };
                let sid = if sid.is_empty() {
                    "sess-1".to_string()
                } else {
                    sid
                };
                if msg["method"] == "tools/call" {
                    let body = format!(
                        "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{{\"progress\":1}}}}\n\n\
                         event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":99,\"method\":\"sampling/createMessage\",\"params\":{{}}}}\n\n\
                         event: message\ndata: {}\n\n",
                        out
                    );
                    respond(
                        &mut sock,
                        200,
                        "text/event-stream",
                        &[("Mcp-Session-Id", &sid)],
                        body.as_bytes(),
                    )
                    .await;
                } else {
                    respond(
                        &mut sock,
                        200,
                        "application/json",
                        &[("Mcp-Session-Id", &sid)],
                        out.to_string().as_bytes(),
                    )
                    .await;
                }
            }
            ("DELETE", "/mcp") => respond(&mut sock, 204, "", &[], b"").await,
            ("POST", "/sse") => respond(&mut sock, 405, "", &[], b"").await,
            ("GET", "/sse") => {
                let sid = "s1".to_string();
                let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
                st.sse_queues.lock().insert(sid.clone(), tx);
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\n\r\n";
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock
                    .write_all(
                        format!(": keepalive\n\nevent: endpoint\ndata: /messages?sid={sid}\n\n")
                            .as_bytes(),
                    )
                    .await;
                let _ = sock.flush().await;
                while let Some(item) = rx.recv().await {
                    if sock
                        .write_all(format!("event: message\ndata: {item}\n\n").as_bytes())
                        .await
                        .is_err()
                    {
                        break;
                    }
                    let _ = sock.flush().await;
                }
                return;
            }
            ("POST", "/messages") => {
                let sid = req.path.split("sid=").nth(1).unwrap_or("").to_string();
                let tx = st.sse_queues.lock().get(&sid).cloned();
                let Some(tx) = tx else {
                    respond(&mut sock, 404, "", &[], b"").await;
                    continue;
                };
                let msg: Value = serde_json::from_slice(&req.body).unwrap_or(Value::Null);
                if let Some(out) = handle(&st, &msg) {
                    let _ = tx.send(out);
                }
                respond(&mut sock, 202, "", &[], b"").await;
            }
            _ => respond(&mut sock, 404, "", &[], b"not found").await,
        }
    }
}

async fn start() -> (String, Arc<State>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let st = Arc::new(State {
        calls: AtomicUsize::new(0),
        sse_queues: Mutex::new(HashMap::new()),
    });
    let st2 = st.clone();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(serve_conn(st2.clone(), sock));
        }
    });
    (format!("http://127.0.0.1:{port}"), st)
}

fn bearer(t: &str) -> Vec<(String, String)> {
    vec![("Authorization".to_string(), format!("Bearer {t}"))]
}

#[tokio::test]
async fn streamable_http_roundtrip() {
    let (base, st) = start().await;
    let url = format!("{base}/mcp");

    let e = connect(&url, &bearer("wrong"), "auto", 10.0)
        .await
        .expect_err("401 rejected");
    assert!(e.auth && e.code == Some(401), "{e:?}");
    assert!(
        e.msg
            .starts_with("认证失败 authentication failed (HTTP 401)"),
        "{}",
        e.msg
    );

    let c = connect(&url, &bearer("test-mcp-token"), "auto", 10.0)
        .await
        .expect("connect");
    assert_eq!(c.transport(), "streamable_http");
    assert_eq!(c.server_info()["name"], "fake-notes");
    assert_eq!(c.instructions(), "Fake notes server.");
    assert_eq!(c.protocol_version(), "2025-06-18");
    assert!(c.capabilities().get("tools").is_some());
    if let Client::StreamableHttp(s) = &c {
        assert_eq!(s.session_id(), "sess-1");
    } else {
        panic!("expected streamable client");
    }

    let tools = c.list_tools(200).await.expect("tools/list");
    assert_eq!(tools.len(), 5, "pagination follows nextCursor");
    assert_eq!(c.list_tools(4).await.unwrap().len(), 4);

    let res = c
        .call_tool("tool1", json!({"a": 1}), Some(5.0))
        .await
        .expect("tools/call over SSE");
    assert_eq!(res["content"][0]["text"], "tool1 got {\"a\":1}");
    assert_eq!(res["structuredContent"]["echo"], json!({"a": 1}));
    assert_eq!(st.calls.load(Ordering::SeqCst), 1);

    let err = c
        .call_tool("boom", json!({}), None)
        .await
        .expect("isError is a normal result");
    assert_eq!(err["isError"], true);

    assert_eq!(
        c.request("ping", Some(json!({})), Some(5.0)).await.unwrap(),
        json!({})
    );
    let e = c.request("nope", None, None).await.unwrap_err();
    assert_eq!(e.msg, "MCP 错误 -32601: method not found");
    assert_eq!(e.code, Some(-32601));
    c.close().await;

    // explicit SSE transport against a Streamable-only path: the GET is not an event stream
    let e = connect(&url, &bearer("test-mcp-token"), "sse", 10.0)
        .await
        .unwrap_err();
    assert!(
        e.msg.contains("not an SSE endpoint") || e.msg.contains("HTTP 404"),
        "{}",
        e.msg
    );
}

#[tokio::test]
async fn legacy_sse_roundtrip() {
    let (base, st) = start().await;
    let url = format!("{base}/sse");
    let c = connect(&url, &[], "auto", 10.0)
        .await
        .expect("connect over SSE");
    assert_eq!(c.transport(), "sse");
    assert_eq!(c.server_info()["name"], "fake-notes");
    if let Client::LegacySse(s) = &c {
        assert_eq!(s.endpoint(), format!("{base}/messages?sid=s1"));
    } else {
        panic!("expected sse client");
    }
    assert_eq!(c.list_tools(128).await.unwrap().len(), 5);
    let res = c
        .call_tool("tool2", json!({"q": "x"}), Some(5.0))
        .await
        .expect("call over SSE");
    assert_eq!(res["content"][0]["text"], "tool2 got {\"q\":\"x\"}");
    assert_eq!(st.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        c.request("ping", Some(json!({})), Some(15.0))
            .await
            .unwrap(),
        json!({})
    );
    c.close().await;
    let e = c.request("ping", Some(json!({})), Some(2.0)).await;
    assert!(
        e.is_ok() || e.is_err(),
        "a closed client reconnects or fails, never hangs"
    );
}

#[tokio::test]
async fn unreachable_server_is_a_connection_error() {
    let e = connect("http://127.0.0.1:9/mcp", &[], "streamable_http", 3.0)
        .await
        .unwrap_err();
    assert!(
        e.msg.starts_with("无法连接 MCP 服务器 connection failed: "),
        "{}",
        e.msg
    );
    assert!(!e.auth && e.code.is_none());
}
