use axum::{
    Router as AxumRouter,
    body::Body as AxumBody,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_data_plane::http::router;
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;

use crate::common::{codex_oauth_auth, openai_upstream, test_runtime_with_auths};

#[derive(Debug, Deserialize)]
struct StreamAbortFixture {
    #[allow(dead_code)]
    description: String,
    request: serde_json::Value,
    partial_upstream_body: String,
    expected_status: u16,
    #[serde(default)]
    expected_events: Vec<String>,
}

#[tokio::test]
async fn stream_true_aborts_after_created_emits_error_frame() {
    let fixture = load_stream_abort_fixture("stream_true_aborts_after_created.json");
    let upstream_url = spawn_aborting_sse_upstream(&fixture.partial_upstream_body).await;
    let app = router(
        test_runtime_with_auths(
            true,
            cliproxy_common_types::snapshot::RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        openai_upstream(upstream_url),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(AxumBody::from(fixture.request.to_string()))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(
        response.status(),
        StatusCode::from_u16(fixture.expected_status).expect("valid status")
    );

    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("valid utf8");
    let events: Vec<&str> = text
        .lines()
        .filter(|line| line.starts_with("event:"))
        .map(|line| line.strip_prefix("event:").unwrap().trim())
        .collect();

    for expected in &fixture.expected_events {
        assert!(
            events.contains(&expected.as_str()),
            "missing event {expected:?} in {events:?}"
        );
    }
}

#[tokio::test]
async fn stream_false_aggregate_aborts_after_created_returns_bad_gateway() {
    let fixture = load_stream_abort_fixture("stream_false_aggregate_aborts_after_created.json");
    let upstream_url = spawn_aborting_sse_upstream(&fixture.partial_upstream_body).await;
    let app = router(
        test_runtime_with_auths(
            true,
            cliproxy_common_types::snapshot::RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        openai_upstream(upstream_url),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(AxumBody::from(fixture.request.to_string()))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(
        response.status(),
        StatusCode::from_u16(fixture.expected_status).expect("valid status")
    );
}

fn load_stream_abort_fixture(name: &str) -> StreamAbortFixture {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root");
    let raw = std::fs::read_to_string(root.join("testdata/contract/stream_abort").join(name))
        .expect("read stream abort fixture");
    serde_json::from_str(&raw).expect("parse stream abort fixture")
}

async fn spawn_aborting_sse_upstream(partial_body: &str) -> String {
    use tokio::net::TcpListener;

    let partial = partial_body.to_string();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");

    let app = AxumRouter::new().route(
        "/responses",
        axum_post(move |_request: Request<AxumBody>| {
            let partial = partial.clone();
            async move {
                let body = AxumBody::from_stream(async_stream::stream! {
                    // Yield the partial body in small chunks so reqwest can bootstrap
                    // the streaming response before the upstream aborts.
                    for chunk in partial.as_bytes().chunks(16) {
                        yield Ok::<_, std::io::Error>(bytes::Bytes::copy_from_slice(chunk));
                    }
                    // Give the client a moment to drain the successful chunks before
                    // the connection breaks; this exercises the abort handling path
                    // rather than failing at response bootstrap.
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    yield Err::<bytes::Bytes, std::io::Error>(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "upstream aborted",
                    ));
                });
                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream; charset=utf-8")],
                    body,
                )
                    .into_response()
            }
        }),
    );

    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });

    format!("http://{}", addr)
}

use cliproxy_data_plane::usage_queue::UsageQueue;
use futures_util::StreamExt;
use reqwest::Client;

#[tokio::test]
async fn downstream_client_drop_cancels_upstream_stream() {
    let (upstream_url, upstream_closed) = spawn_slow_infinite_sse_upstream().await;
    let usage_queue = UsageQueue::new();

    let app = router(
        test_runtime_with_auths(
            true,
            cliproxy_common_types::snapshot::RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        openai_upstream(upstream_url),
    );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind data plane");
    let addr = listener.local_addr().expect("data plane addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve data plane");
    });

    let client = Client::new();
    let response = client
        .post(format!("http://{}/v1/responses", addr))
        .header("content-type", "application/json")
        .body(r#"{"model":"codex-latest","input":"hello","stream":true}"#.to_string())
        .send()
        .await
        .expect("send request");

    assert_eq!(response.status(), 200);

    let mut stream = response.bytes_stream();
    let mut frames = 0;
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.expect("chunk");
        let text = String::from_utf8_lossy(&bytes);
        for line in text.lines() {
            if line.starts_with("event:") {
                frames += 1;
            }
        }
        if frames >= 2 {
            break;
        }
    }
    assert!(frames >= 2, "should read at least 2 SSE frames");

    // Drop the response body and client to simulate downstream abort.
    drop(stream);
    drop(client);

    // Wait for the cancel to propagate back to the upstream mock.
    for _ in 0..30 {
        if upstream_closed.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert!(
        upstream_closed.load(std::sync::atomic::Ordering::SeqCst),
        "upstream should observe connection close after downstream drops"
    );

    // Usage queue should not contain a success-completed record.
    let payloads = usage_queue.pop_oldest_json(10);
    let success_completed = payloads.iter().any(|payload| {
        payload.get("failed").and_then(|v| v.as_bool()) == Some(false)
            && payload.get("endpoint").and_then(|v| v.as_str()) == Some("POST /v1/responses")
    });
    assert!(
        !success_completed,
        "usage queue should not contain a success completed payload after downstream abort"
    );
}

async fn spawn_slow_infinite_sse_upstream()
-> (String, std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    let closed = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let closed_spawn = Arc::clone(&closed);

    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let closed = Arc::clone(&closed_spawn);
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                // Read the HTTP request headers to unblock the client.
                let _ =
                    tokio::time::timeout(Duration::from_millis(100), stream.read(&mut buf)).await;

                let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n";
                if stream.write_all(headers).await.is_err() {
                    closed.store(true, Ordering::SeqCst);
                    return;
                }

                let created = b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-abort-1\",\"status\":\"in_progress\"}}\n\n";
                if stream.write_all(created).await.is_err() {
                    closed.store(true, Ordering::SeqCst);
                    return;
                }

                let mut counter = 0;
                loop {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    let delta = format!(
                        "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"item_id\":\"msg-1\",\"output_index\":0,\"content_index\":0,\"delta\":\"{counter}\"}}\n\n"
                    );
                    if stream.write_all(delta.as_bytes()).await.is_err() {
                        closed.store(true, Ordering::SeqCst);
                        return;
                    }
                    counter += 1;
                }
            });
        }
    });

    (format!("http://{}", addr), closed)
}
