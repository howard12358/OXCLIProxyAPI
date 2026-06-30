mod common;

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use cliproxy_common_types::snapshot::RoutingStrategy;
use cliproxy_data_plane::http::{router, router_with_snapshot_client};
use cliproxy_runtime_config_client::{
    RuntimeConfigClient, RuntimeConfigClientConfig, SnapshotSource,
};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tempfile::NamedTempFile;
use tower::ServiceExt;

use common::{
    codex_oauth_auth, codex_upstream, openai_upstream, spawn_codex_failover_upstream,
    spawn_codex_quota_failover_upstream, spawn_openai_upstream, test_runtime,
    test_runtime_with_auths, test_upstream,
};

async fn spawn_split_sse_openai_upstream() -> String {
    use axum::{
        Router as AxumRouter,
        body::Body as AxumBody,
        http::{HeaderMap, Request as AxumRequest, StatusCode as AxumStatusCode},
        response::IntoResponse,
        routing::post as axum_post,
    };
    use tokio::net::TcpListener;

    async fn responses(_headers: HeaderMap, _request: AxumRequest<AxumBody>) -> impl IntoResponse {
        (
            AxumStatusCode::OK,
            [("content-type", "text/event-stream; charset=utf-8")],
            AxumBody::from(concat!(
                "event: response.created",
                "\ndata: {\"type\":\"response.created\",\"response\":{\"id\":\"resp-stream-1\",\"status\":\"in_progress\"}}",
                "\n\n",
                "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc-1\",\"call_id\":\"call-1\",\"name\":\"shell\",\"arguments\":\"{}\"}}\n\n",
                "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-stream-1\",\"output\":[]}}"
            )),
        )
            .into_response()
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = AxumRouter::new().route("/responses", axum_post(responses));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}

fn sse_data_payload(frame: &str) -> Option<String> {
    let lines = frame
        .lines()
        .filter_map(|line| line.trim().strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>();
    (!lines.is_empty()).then(|| lines.join("\n"))
}

#[tokio::test]
async fn responses_route_returns_not_found_when_disabled() {
    let app = router(test_runtime(false), test_upstream());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":true,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn snapshot_notify_triggers_runtime_refresh() {
    let runtime = test_runtime(true);
    let mut next_snapshot = runtime
        .current_snapshot()
        .expect("snapshot")
        .as_ref()
        .clone();
    next_snapshot.version = "test-v2".to_string();
    next_snapshot.network.upstream_proxy = Some("socks5h://127.0.0.1:7897".to_string());

    let snapshot_file = NamedTempFile::new().expect("create snapshot file");
    std::fs::write(
        snapshot_file.path(),
        serde_json::to_vec(&next_snapshot).expect("serialize snapshot"),
    )
    .expect("write snapshot file");

    let snapshot_client = RuntimeConfigClient::new(RuntimeConfigClientConfig {
        source: SnapshotSource::File {
            path: snapshot_file.path().to_path_buf(),
        },
        poll_interval_seconds: 30,
    });
    let app = router_with_snapshot_client(runtime.clone(), test_upstream(), Some(snapshot_client));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v0/runtime/snapshot-notify")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "version":"test-v2",
                        "generated_at":"2026-06-22T10:00:00Z",
                        "reason":"runtime_snapshot_changed"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::ACCEPTED);

    for _ in 0..50 {
        if runtime.current_snapshot_version().as_deref() == Some("test-v2") {
            break;
        }
        tokio::task::yield_now().await;
    }

    assert_eq!(
        runtime.current_snapshot_version().as_deref(),
        Some("test-v2")
    );
    let snapshot = runtime.current_snapshot().expect("updated snapshot");
    assert_eq!(
        snapshot.network.upstream_proxy.as_deref(),
        Some("socks5h://127.0.0.1:7897")
    );
}

#[tokio::test]
async fn runtime_snapshot_endpoint_returns_applied_snapshot() {
    let app = router(test_runtime(true), test_upstream());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v0/runtime/snapshot")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["version"], "test-v1");
    assert_eq!(payload["routes"]["responses"], true);
    assert_eq!(payload["providers"]["codex"]["enabled"], true);
    assert_eq!(payload["auth_pool"][0]["id"], "auth-codex-1");
}

#[tokio::test]
async fn responses_route_returns_bad_gateway_when_no_real_upstream_is_available() {
    let app = router(test_runtime(true), test_upstream());
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":true,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["error"]["code"], "upstream_unavailable");
}

#[tokio::test]
async fn responses_non_streaming_prefers_real_openai_upstream() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":false,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["provider"], "openai");
    assert_eq!(payload["auth"], "Bearer openai-key");
}

#[tokio::test]
async fn responses_route_resolves_codex_alias_before_upstream_execution() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":false,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["model"], "gpt-5-codex");
}

#[tokio::test]
async fn responses_streaming_prefers_real_openai_upstream() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":true,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("valid utf8");
    assert!(text.contains("\"provider\":\"openai\""));
    assert!(text.contains("Bearer openai-key"));
}

#[tokio::test]
async fn responses_stream_repairs_completed_output_from_split_upstream_frames() {
    let upstream_url = spawn_split_sse_openai_upstream().await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"model":"codex-latest","stream":true,"input":"hello"}).to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("valid utf8");
    assert!(text.contains("event: response.created"));
    assert!(text.contains("\"response.output_item.done\""));
    assert!(text.contains("\"response.completed\""));
    let completed_payload = text
        .split("\n\n")
        .find_map(|frame| {
            let payload = sse_data_payload(frame)?;
            payload
                .contains("\"type\":\"response.completed\"")
                .then_some(payload)
        })
        .expect("completed payload");
    let completed: Value = serde_json::from_str(&completed_payload).expect("parse completed");
    assert_eq!(completed["response"]["output"][0]["id"], "fc-1");
    assert_eq!(completed["response"]["output"][0]["name"], "shell");
}

#[tokio::test]
async fn responses_route_executes_selected_codex_oauth_auth_end_to_end() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::RoundRobin,
            vec![
                codex_oauth_auth(
                    "auth-codex-a",
                    100,
                    "codex-token-a",
                    "acct_a",
                    Some(&upstream_url),
                ),
                codex_oauth_auth(
                    "auth-codex-b",
                    100,
                    "codex-token-b",
                    "acct_b",
                    Some(&upstream_url),
                ),
            ],
        ),
        codex_upstream(upstream_url),
    );

    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"hello",
                        "metadata":{"user_id":"{\"device_id\":\"d1\",\"account_uuid\":\"\",\"session_id\":\"session-1\"}"}
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");
    let first_body = first
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let first_payload: Value = serde_json::from_slice(&first_body).expect("parse body");
    assert_eq!(first_payload["auth"], "Bearer codex-token-a");
    assert_eq!(first_payload["account_id"], "acct_a");

    let second = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"next",
                        "metadata":{"user_id":"{\"device_id\":\"d1\",\"account_uuid\":\"\",\"session_id\":\"session-1\"}"}
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");
    let second_body = second
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let second_payload: Value = serde_json::from_slice(&second_body).expect("parse body");
    assert_eq!(second_payload["auth"], "Bearer codex-token-a");

    let third = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"other",
                        "metadata":{"user_id":"{\"device_id\":\"d2\",\"account_uuid\":\"\",\"session_id\":\"session-2\"}"}
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");
    let third_body = third
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let third_payload: Value = serde_json::from_slice(&third_body).expect("parse body");
    assert_eq!(third_payload["auth"], "Bearer codex-token-b");
    assert_eq!(third_payload["account_id"], "acct_b");
}

#[tokio::test]
async fn responses_route_executes_auth_bound_codex_upstream_without_global_token() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        test_upstream(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"hello"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["object"], "response");
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["provider"], "openai");
    assert_eq!(payload["auth"], "Bearer codex-token-a");
    assert_eq!(payload["account_id"], "acct_a");
}

#[tokio::test]
async fn responses_route_fails_over_on_codex_quota_exhaustion() {
    let upstream_url = spawn_codex_quota_failover_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::FillFirst,
            vec![
                codex_oauth_auth(
                    "auth-codex-a",
                    100,
                    "codex-token-a",
                    "acct_a",
                    Some(&upstream_url),
                ),
                codex_oauth_auth(
                    "auth-codex-b",
                    100,
                    "codex-token-b",
                    "acct_b",
                    Some(&upstream_url),
                ),
            ],
        ),
        test_upstream(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"quota failover"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["auth"], "Bearer codex-token-b");
}

#[tokio::test]
async fn responses_route_normalizes_codex_payload_for_upstream() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        test_upstream(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":true,
                        "input":"hello from codex",
                        "metadata":{"client":"codex-test"}
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let text = String::from_utf8(body.to_vec()).expect("valid utf8");
    let completed_line = text
        .lines()
        .find(|line| line.starts_with("data: {\"type\":\"response.completed\""))
        .expect("completed event");
    let payload: Value = serde_json::from_str(
        completed_line
            .strip_prefix("data: ")
            .expect("completed payload prefix"),
    )
    .expect("parse completed payload");
    let received = &payload["response"]["received_payload"];
    assert_eq!(payload["response"]["auth"], "Bearer codex-token-a");
    assert_eq!(received["store"], false);
    assert_eq!(
        received["instructions"]
            .as_str()
            .expect("instructions string"),
        "You are Codex. Fulfill the user's request."
    );
    assert_eq!(received["input"][0]["role"], "user");
    assert_eq!(received["input"][0]["content"][0]["type"], "input_text");
    assert_eq!(
        received["input"][0]["content"][0]["text"],
        "hello from codex"
    );
    assert!(received.get("metadata").is_none());
}

#[tokio::test]
async fn responses_route_aggregates_codex_stream_for_non_stream_clients() {
    let upstream_url = spawn_openai_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::FillFirst,
            vec![codex_oauth_auth(
                "auth-codex-a",
                100,
                "codex-token-a",
                "acct_a",
                Some(&upstream_url),
            )],
        ),
        test_upstream(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"aggregate this"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["object"], "response");
    assert_eq!(payload["status"], "completed");
    assert_eq!(payload["provider"], "openai");
    assert_eq!(payload["auth"], "Bearer codex-token-a");
    assert_eq!(payload["account_id"], "acct_a");
    assert_eq!(payload["model"], "gpt-5-codex");
    assert_eq!(payload["output"][0]["content"][0]["text"], "stream ok");
}

#[tokio::test]
async fn responses_route_retries_next_auth_after_retryable_codex_failure() {
    let upstream_url = spawn_codex_failover_upstream().await;
    let app = router(
        test_runtime_with_auths(
            true,
            RoutingStrategy::FillFirst,
            vec![
                codex_oauth_auth(
                    "auth-codex-a",
                    100,
                    "codex-token-a",
                    "acct_a",
                    Some(&upstream_url),
                ),
                codex_oauth_auth(
                    "auth-codex-b",
                    100,
                    "codex-token-b",
                    "acct_b",
                    Some(&upstream_url),
                ),
            ],
        ),
        test_upstream(),
    );

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({
                        "model":"codex-latest",
                        "stream":false,
                        "input":"retry please"
                    })
                    .to_string(),
                ))
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::OK);
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["auth"], "Bearer codex-token-b");
    assert_eq!(payload["object"], "response");
    assert_eq!(payload["status"], "completed");
}

#[tokio::test]
async fn metrics_endpoint_is_not_exposed() {
    let app = router(test_runtime(true), test_upstream());
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("call app");

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
