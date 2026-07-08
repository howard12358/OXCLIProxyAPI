use axum::{
    body::Body as AxumBody,
    http::{Request, StatusCode},
};
use cliproxy_common_types::snapshot::{RoutingStrategy, UsageQueueConfig};
use cliproxy_data_plane::{
    auth_state::AuthStateOverlay, http::router_with_snapshot_client_and_usage_queue,
    redis_protocol, usage_queue::UsageQueue,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Duration, timeout},
};
use tower::ServiceExt;

use crate::common::{
    codex_oauth_auth, spawn_openai_upstream, test_runtime_with_auths, test_upstream,
};

#[tokio::test]
async fn http_usage_queue_pop_is_fifo_and_consuming() {
    let upstream_url = spawn_openai_upstream().await;
    let runtime = test_runtime_with_auths(
        true,
        RoutingStrategy::FillFirst,
        vec![codex_oauth_auth(
            "auth-codex-a",
            100,
            "codex-token-a",
            "acct_a",
            Some(&upstream_url),
        )],
    );
    let mut snapshot = runtime
        .current_snapshot()
        .expect("snapshot")
        .as_ref()
        .clone();
    snapshot.usage_queue = UsageQueueConfig {
        enabled: true,
        backend: "redis".to_string(),
    };
    runtime.apply_snapshot(snapshot);

    let usage_queue = UsageQueue::new();
    let app = router_with_snapshot_client_and_usage_queue(
        runtime,
        test_upstream(),
        None,
        usage_queue,
        AuthStateOverlay::new(),
    );

    send_success_response_request(app.clone(), "req-1").await;
    send_success_response_request(app.clone(), "req-2").await;

    let first = pop_http_usage(app.clone(), 1).await;
    let second = pop_http_usage(app.clone(), 1).await;
    let empty = pop_http_usage(app, 1).await;

    assert_eq!(first.as_array().expect("first array").len(), 1);
    assert_eq!(first[0]["request_id"], "req-1");
    assert_eq!(second.as_array().expect("second array").len(), 1);
    assert_eq!(second[0]["request_id"], "req-2");
    assert!(empty.as_array().expect("empty array").is_empty());
}

#[tokio::test]
async fn resp_lpop_rpop_and_auth_follow_usage_contract() {
    let queue = UsageQueue::new();
    queue.enqueue_raw(br#"{"request_id":"req-1","model":"gpt-5-codex"}"#.to_vec());
    queue.enqueue_raw(br#"{"request_id":"req-2","model":"gpt-5-codex"}"#.to_vec());
    let addr = spawn_redis_protocol(queue, Some("secret".to_string())).await;

    let mut wrong = TcpStream::connect(addr).await.expect("connect wrong");
    wrong
        .write_all(resp_command(&["AUTH", "wrong"]).as_bytes())
        .await
        .expect("write wrong auth");
    let wrong_response = read_resp_text_until(&mut wrong, "-ERR invalid password").await;
    assert!(wrong_response.contains("-ERR invalid password"));

    let mut client = TcpStream::connect(addr).await.expect("connect client");
    client
        .write_all(resp_command(&["AUTH", "secret"]).as_bytes())
        .await
        .expect("write auth");
    assert!(
        read_resp_text_until(&mut client, "+OK")
            .await
            .contains("+OK")
    );

    client
        .write_all(resp_command(&["LPOP", "usage"]).as_bytes())
        .await
        .expect("write lpop");
    let first = read_resp_text_until(&mut client, "req-1").await;
    assert_eq!(parse_bulk_json(&first)["request_id"], "req-1");

    client
        .write_all(resp_command(&["RPOP", "usage"]).as_bytes())
        .await
        .expect("write rpop");
    let second = read_resp_text_until(&mut client, "req-2").await;
    assert_eq!(parse_bulk_json(&second)["request_id"], "req-2");

    client
        .write_all(resp_command(&["LPOP", "usage"]).as_bytes())
        .await
        .expect("write empty lpop");
    assert!(
        read_resp_text_until(&mut client, "$-1")
            .await
            .contains("$-1")
    );
}

#[tokio::test]
async fn resp_subscribe_receives_usage_without_buffering_http_fallback_copy() {
    let queue = UsageQueue::new();
    let addr = spawn_redis_protocol(queue.clone(), Some("secret".to_string())).await;
    let mut client = TcpStream::connect(addr).await.expect("connect client");

    client
        .write_all(resp_command(&["AUTH", "secret"]).as_bytes())
        .await
        .expect("write auth");
    assert!(
        read_resp_text_until(&mut client, "+OK")
            .await
            .contains("+OK")
    );
    client
        .write_all(resp_command(&["SUBSCRIBE", "usage"]).as_bytes())
        .await
        .expect("write subscribe");
    let subscribed = read_resp_text_until(&mut client, r#"{"support_refresh":true}"#).await;
    assert!(subscribed.contains("subscribe"));
    assert!(subscribed.contains(r#"{"support_refresh":true}"#));

    queue.enqueue_raw(br#"{"request_id":"req-sub"}"#.to_vec());
    let message = read_resp_text_until(&mut client, "req-sub").await;
    assert!(message.contains("message"));
    assert_eq!(parse_last_bulk_json(&message)["request_id"], "req-sub");
    assert!(queue.pop_oldest_json(1).is_empty());
}

async fn send_success_response_request(app: axum::Router, request_id: &'static str) {
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .header("x-request-id", request_id)
                .body(AxumBody::from(
                    json!({
                        "model": "codex-latest",
                        "stream": true,
                        "input": "hello"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");
    assert_eq!(response.status(), StatusCode::OK);
    let _ = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
}

async fn pop_http_usage(app: axum::Router, count: usize) -> Value {
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/v0/management/usage-queue?count={count}"))
                .body(AxumBody::empty())
                .expect("build request"),
        )
        .await
        .expect("call usage queue");
    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    serde_json::from_slice(&body).expect("parse usage queue response")
}

async fn spawn_redis_protocol(
    queue: UsageQueue,
    auth_password: Option<String>,
) -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.expect("accept");
            let queue = queue.clone();
            let auth_password = auth_password.clone();
            tokio::spawn(async move {
                redis_protocol::handle_connection(stream, queue, auth_password)
                    .await
                    .expect("handle connection");
            });
        }
    });
    addr
}

fn resp_command(args: &[&str]) -> String {
    let mut out = format!("*{}\r\n", args.len());
    for arg in args {
        out.push_str(&format!("${}\r\n{}\r\n", arg.len(), arg));
    }
    out
}

async fn read_resp_text_until(stream: &mut TcpStream, needle: &str) -> String {
    let mut out = String::new();
    let mut buf = [0_u8; 512];
    timeout(Duration::from_secs(2), async {
        while !out.contains(needle) {
            let n = stream.read(&mut buf).await.expect("read response");
            assert!(
                n > 0,
                "connection closed before {needle:?}; response={out:?}"
            );
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
        }
    })
    .await
    .expect("timed out reading RESP response");
    out
}

fn parse_bulk_json(response: &str) -> Value {
    let start = response.find('{').expect("json start");
    let end = response.rfind('}').expect("json end");
    serde_json::from_str(&response[start..=end]).expect("parse bulk json")
}

fn parse_last_bulk_json(response: &str) -> Value {
    let start = response.rfind('{').expect("json start");
    let end = response[start..]
        .find('}')
        .map(|offset| start + offset)
        .expect("json end");
    serde_json::from_str(&response[start..=end]).expect("parse bulk json")
}
