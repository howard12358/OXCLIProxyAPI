use std::sync::{Arc, Mutex};

use axum::{
    Router as AxumRouter,
    body::Body as AxumBody,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_common_types::snapshot::{ExternalUsageQueueConfig, UsageQueueConfig};
use cliproxy_data_plane::http::router_with_snapshot_client_and_usage_queue;
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tower::ServiceExt;

use crate::common::{codex_oauth_auth, test_runtime_with_auths, test_upstream};

#[tokio::test]
async fn home_mode_lpush_usage_to_external_redis_queue() {
    let captured = Arc::new(Mutex::new(Vec::<String>::new()));
    let external_addr = spawn_fake_external_usage_redis(Arc::clone(&captured)).await;

    let usage_queue = cliproxy_data_plane::usage_queue::UsageQueue::new();
    usage_queue.set_external_config(Some(ExternalUsageQueueConfig {
        address: external_addr.clone(),
        password: String::from("usage-secret"),
        key: String::from("usage"),
        timeout_ms: 5000,
    }));

    let upstream_url = spawn_codex_upstream_for_usage().await;
    let runtime = test_runtime_with_auths(
        true,
        cliproxy_common_types::snapshot::RoutingStrategy::FillFirst,
        vec![codex_oauth_auth(
            "auth-codex-a",
            100,
            "codex-token-a",
            "acct_a",
            Some(&upstream_url),
        )],
    );

    // Patch runtime snapshot usage_queue to enable usage telemetry.
    {
        let mut snapshot = runtime
            .current_snapshot()
            .expect("snapshot")
            .as_ref()
            .clone();
        snapshot.usage_queue = UsageQueueConfig {
            enabled: true,
            backend: String::from("redis"),
            external: Some(ExternalUsageQueueConfig {
                address: external_addr,
                password: String::from("usage-secret"),
                key: String::from("usage"),
                timeout_ms: 5000,
            }),
        };
        runtime.apply_snapshot(snapshot);
    }

    let app = router_with_snapshot_client_and_usage_queue(
        runtime,
        test_upstream(),
        None,
        usage_queue,
        cliproxy_data_plane::auth_state::AuthStateOverlay::new(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(AxumBody::from(
                    serde_json::json!({
                        "model": "codex-latest",
                        "input": "hello",
                        "stream": false,
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);

    // Wait for the asynchronous LPUSH to reach the fake Redis server.
    for _ in 0..50 {
        {
            let guard = captured.lock().expect("lock captured commands");
            if guard.iter().any(|line| line.starts_with("LPUSH usage ")) {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let guard = captured.lock().expect("lock captured commands");
    let commands: Vec<String> = guard.clone();
    drop(guard);

    let auth_ok = commands.iter().any(|line| line == "AUTH usage-secret");
    assert!(auth_ok, "fake redis should receive AUTH command");

    let lpush_line = commands
        .iter()
        .find(|line| line.starts_with("LPUSH usage "))
        .expect("fake redis should receive LPUSH usage command");
    let payload_line = lpush_line
        .strip_prefix("LPUSH usage ")
        .expect("LPUSH should have a payload argument");
    let payload: Value = serde_json::from_str(payload_line).expect("payload should be valid JSON");

    assert_eq!(payload["provider"], "codex");
    assert_eq!(payload["model"], "gpt-5-codex");
    assert_eq!(payload["source"], "auth-codex-a@example.test");
}

async fn spawn_fake_external_usage_redis(captured: Arc<Mutex<Vec<String>>>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind fake redis");
    let addr = listener.local_addr().expect("fake redis addr");

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let captured = Arc::clone(&captured);
            tokio::spawn(async move {
                let mut authenticated = false;
                let mut buf = Vec::with_capacity(4096);
                loop {
                    // RESP arrays for AUTH/LPUSH fit in a single read.
                    let mut read_buf = [0u8; 4096];
                    let n = match stream.read(&mut read_buf).await {
                        Ok(0) => break,
                        Ok(n) => n,
                        Err(_) => break,
                    };
                    buf.extend_from_slice(&read_buf[..n]);

                    let mut consumed = 0;
                    while let Some((command, next_offset)) = parse_resp_array(&buf[consumed..]) {
                        consumed += next_offset;
                        if command.is_empty() {
                            break;
                        }
                        let joined = command.join(" ");
                        captured.lock().expect("lock").push(joined);

                        let cmd_upper = command[0].to_ascii_uppercase();
                        if cmd_upper == "AUTH" {
                            authenticated = true;
                            let _ = stream.write_all(b"+OK\r\n").await;
                        } else if cmd_upper == "LPUSH" && authenticated {
                            let _ = stream.write_all(b":1\r\n").await;
                        } else {
                            let _ = stream.write_all(b"-ERR unknown command\r\n").await;
                        }
                    }
                    if consumed > 0 {
                        buf.drain(..consumed);
                    }
                }
            });
        }
    });

    format!("127.0.0.1:{}", addr.port())
}

/// Parse one RESP array from `data` and return (args, bytes_consumed).
/// Supports only the simple array-of-bulk-strings used by AUTH/LPUSH.
fn parse_resp_array(data: &[u8]) -> Option<(Vec<String>, usize)> {
    if data.is_empty() {
        return None;
    }
    if data[0] != b'*' {
        return None;
    }
    let mut offset = 0;
    let count = read_line_integer(data, &mut offset)?;
    if count < 0 {
        return Some((Vec::new(), offset));
    }
    let mut args = Vec::with_capacity(count as usize);
    for _ in 0..count {
        if data.get(offset)? != &b'$' {
            return None;
        }
        let len = read_line_integer(data, &mut offset)?;
        if len < 0 {
            args.push(String::new());
            continue;
        }
        let len = len as usize;
        if data.len() < offset + len + 2 {
            return None; // need more data
        }
        let arg = String::from_utf8_lossy(&data[offset..offset + len]).into_owned();
        offset += len;
        if &data[offset..offset + 2] != b"\r\n" {
            return None;
        }
        offset += 2;
        args.push(arg);
    }
    Some((args, offset))
}

fn read_line_integer(data: &[u8], offset: &mut usize) -> Option<i64> {
    let start = *offset;
    let newline = data[start..].iter().position(|&b| b == b'\n')?;
    let line = &data[start..start + newline];
    *offset = start + newline + 1;
    // Strip optional \r.
    let line = if line.last() == Some(&b'\r') {
        &line[..line.len() - 1]
    } else {
        line
    };
    let first = *line.first()?;
    std::str::from_utf8(&line[1..])
        .ok()?
        .parse::<i64>()
        .ok()
        .map(|v| if first == b'-' { -v } else { v })
}

async fn spawn_codex_upstream_for_usage() -> String {
    use axum::{
        body::Body,
        http::{HeaderMap, Request},
    };

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");

    let app = AxumRouter::new().route(
        "/responses",
        axum_post(|_headers: HeaderMap, _request: Request<Body>| async move {
            (
                StatusCode::OK,
                [("content-type", "text/event-stream; charset=utf-8")],
                AxumBody::from(concat!(
                    "event: response.completed\n",
                    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-usage-1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}]}}\n\n"
                )),
            )
                .into_response()
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });

    format!("http://{}", addr)
}
