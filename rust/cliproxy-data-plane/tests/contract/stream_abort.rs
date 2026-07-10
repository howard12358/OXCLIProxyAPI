use axum::{
    Router as AxumRouter,
    body::Body as AxumBody,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_common_types::snapshot::UsageQueueConfig;
use cliproxy_data_plane::{
    auth_state::{AuthKey, AuthStateOverlay},
    http::{router, router_with_snapshot_client_and_usage_queue},
};
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

#[tokio::test]
async fn concurrent_downstream_aborts_cancel_only_aborted_upstreams() {
    use std::{sync::atomic::Ordering, time::Duration};
    use tokio::task::JoinSet;

    const REQUEST_COUNT: usize = 10;
    const ABORTED_REQUESTS: usize = REQUEST_COUNT / 2;

    let (upstream_url, upstream_closed, upstream_active, shutdown_upstream) =
        spawn_mixed_sse_upstream().await;
    let usage_queue = UsageQueue::new();
    let auth_state = AuthStateOverlay::new();
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
    let mut snapshot = runtime
        .current_snapshot()
        .expect("snapshot")
        .as_ref()
        .clone();
    snapshot.usage_queue = UsageQueueConfig {
        enabled: true,
        backend: "redis".to_string(),
        external: None,
    };
    runtime.apply_snapshot(snapshot);

    let app = router_with_snapshot_client_and_usage_queue(
        runtime,
        openai_upstream(upstream_url.clone()),
        None,
        usage_queue.clone(),
        auth_state.clone(),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind data plane");
    let addr = listener.local_addr().expect("data plane addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve data plane");
    });

    let mut requests = JoinSet::new();
    for index in 0..REQUEST_COUNT {
        let request_id = format!("concurrent-abort-{index}");
        let abort = index < ABORTED_REQUESTS;
        let url = format!("http://{addr}/v1/responses");
        requests.spawn(async move {
            let client = Client::new();
            let response = client
                .post(url)
                .header("content-type", "application/json")
                .header("x-request-id", &request_id)
                .body(format!(
                    r#"{{"model":"codex-latest","input":"{}-{index}","stream":true}}"#,
                    if abort { "abort" } else { "complete" }
                ))
                .send()
                .await
                .expect("send request");
            assert_eq!(response.status(), StatusCode::OK);

            if abort {
                let mut stream = response.bytes_stream();
                let mut events = 0;
                while let Some(chunk) = stream.next().await {
                    let chunk = chunk.expect("stream chunk");
                    events += String::from_utf8_lossy(&chunk)
                        .lines()
                        .filter(|line| line.starts_with("event:"))
                        .count();
                    if events >= 2 {
                        break;
                    }
                }
                assert!(events >= 2, "aborted client should observe two SSE events");
                drop(stream);
                drop(client);
                (request_id, true)
            } else {
                let body = response.bytes().await.expect("complete stream body");
                assert!(
                    String::from_utf8_lossy(&body).contains("event: response.completed"),
                    "non-aborted stream should complete"
                );
                (request_id, false)
            }
        });
    }

    let mut aborted_ids = Vec::new();
    let mut completed_ids = Vec::new();
    while let Some(result) = requests.join_next().await {
        let (request_id, aborted) = result.expect("request task");
        if aborted {
            aborted_ids.push(request_id);
        } else {
            completed_ids.push(request_id);
        }
    }
    assert_eq!(aborted_ids.len(), ABORTED_REQUESTS);
    assert_eq!(completed_ids.len(), REQUEST_COUNT - ABORTED_REQUESTS);

    for _ in 0..40 {
        if upstream_closed.load(Ordering::SeqCst) == ABORTED_REQUESTS
            && upstream_active.load(Ordering::SeqCst) == 0
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert_eq!(
        upstream_closed.load(Ordering::SeqCst),
        ABORTED_REQUESTS,
        "every downstream abort should close exactly one upstream stream"
    );
    assert_eq!(
        upstream_active.load(Ordering::SeqCst),
        0,
        "all upstream connection tasks should finish after client completion or abort"
    );

    let usage_payloads = usage_queue.pop_oldest_json(REQUEST_COUNT + 2);
    for request_id in &aborted_ids {
        assert!(
            !usage_payloads.iter().any(|payload| {
                payload.get("request_id").and_then(|value| value.as_str()) == Some(request_id)
                    && payload.get("failed").and_then(|value| value.as_bool()) == Some(false)
            }),
            "aborted request {request_id} must not emit a successful usage payload"
        );
    }
    for request_id in &completed_ids {
        assert!(
            usage_payloads.iter().any(|payload| {
                payload.get("request_id").and_then(|value| value.as_str()) == Some(request_id)
                    && payload.get("failed").and_then(|value| value.as_bool()) == Some(false)
            }),
            "completed request {request_id} should emit a successful usage payload"
        );
    }
    assert!(
        auth_state
            .auth_blocked_until(
                &AuthKey::from_auth_record(&codex_oauth_auth(
                    "auth-codex-a",
                    100,
                    "codex-token-a",
                    "acct_a",
                    Some(&upstream_url),
                ))
                .expect("auth key"),
                time::OffsetDateTime::now_utc(),
            )
            .is_none(),
        "client cancellation must not mark auth unhealthy"
    );

    let follow_up = Client::new()
        .post(format!("http://{addr}/v1/responses"))
        .header("content-type", "application/json")
        .body(r#"{"model":"codex-latest","input":"complete-follow-up","stream":true}"#)
        .send()
        .await
        .expect("send follow-up");
    assert_eq!(follow_up.status(), StatusCode::OK);
    assert!(
        String::from_utf8_lossy(&follow_up.bytes().await.expect("follow-up body"))
            .contains("event: response.completed"),
        "a later request should still complete"
    );

    let _ = shutdown_upstream.send(());
}

async fn spawn_mixed_sse_upstream() -> (
    String,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    std::sync::Arc<std::sync::atomic::AtomicUsize>,
    tokio::sync::oneshot::Sender<()>,
) {
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };
    use tokio::{io::AsyncWriteExt, net::TcpListener, sync::oneshot};

    let closed = Arc::new(AtomicUsize::new(0));
    let active = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mixed upstream");
    let addr = listener.local_addr().expect("mixed upstream addr");
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let closed_listener = Arc::clone(&closed);
    let active_listener = Arc::clone(&active);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => break,
                accepted = listener.accept() => {
                    let Ok((mut stream, _)) = accepted else {
                        break;
                    };
                    let closed = Arc::clone(&closed_listener);
                    let active = Arc::clone(&active_listener);
                    tokio::spawn(async move {
                        active.fetch_add(1, Ordering::SeqCst);
                        let Some(request) = read_http_request(&mut stream).await else {
                            active.fetch_sub(1, Ordering::SeqCst);
                            return;
                        };
                        let completes = String::from_utf8_lossy(&request).contains("complete");
                        let headers = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n";
                        let created = b"event: response.created\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-concurrent\",\"status\":\"in_progress\"}}\n\n";
                        if stream.write_all(headers).await.is_err() || stream.write_all(created).await.is_err() {
                            if !completes {
                                closed.fetch_add(1, Ordering::SeqCst);
                            }
                            active.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                        if completes {
                            let completed = b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-concurrent\",\"object\":\"response\",\"status\":\"completed\",\"output\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":1,\"total_tokens\":2}}}\n\n";
                            let _ = stream.write_all(completed).await;
                            active.fetch_sub(1, Ordering::SeqCst);
                            return;
                        }
                        let mut counter = 0;
                        loop {
                            tokio::time::sleep(Duration::from_millis(20)).await;
                            let delta = format!(
                                "event: response.output_text.delta\ndata: {{\"type\":\"response.output_text.delta\",\"delta\":\"{counter}\"}}\n\n"
                            );
                            if stream.write_all(delta.as_bytes()).await.is_err() {
                                closed.fetch_add(1, Ordering::SeqCst);
                                active.fetch_sub(1, Ordering::SeqCst);
                                return;
                            }
                            counter += 1;
                        }
                    });
                }
            }
        }
    });

    (format!("http://{addr}"), closed, active, shutdown_tx)
}

async fn read_http_request(stream: &mut tokio::net::TcpStream) -> Option<Vec<u8>> {
    use tokio::io::AsyncReadExt;

    let mut request = Vec::with_capacity(4096);
    let mut chunk = [0_u8; 4096];
    let mut expected_len = None;

    loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        request.extend_from_slice(&chunk[..read]);

        if expected_len.is_none() {
            let Some(headers_end) = request.windows(4).position(|value| value == b"\r\n\r\n")
            else {
                continue;
            };
            let headers = std::str::from_utf8(&request[..headers_end]).ok()?;
            let content_length = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().ok())
                    .flatten()
            })?;
            expected_len = Some(headers_end + 4 + content_length);
        }

        if request.len() >= expected_len.expect("content length is set") {
            return Some(request);
        }
    }
}
