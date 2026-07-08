use std::sync::{Arc, Mutex};

use axum::{
    Json as AxumJson, Router as AxumRouter,
    body::Body as AxumBody,
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_common_types::snapshot::RoutingStrategy;
use cliproxy_data_plane::auth_state::{AuthKey, AuthStateOverlay};
use cliproxy_data_plane::http::router_with_snapshot_client_and_usage_queue;
use cliproxy_data_plane::usage_queue::UsageQueue;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::{Value, json};
use tower::ServiceExt;

use crate::common::{codex_oauth_auth, test_runtime_with_auths, test_upstream};

#[derive(Debug, Deserialize)]
struct AuthRetryFixture {
    first_status: u16,
    first_body: Value,
    expected_auth: String,
    cooldown_scope: String,
    expect_second_request_skips_primary: bool,
}

#[tokio::test]
async fn retries_next_auth_after_401_fixture() {
    run_auth_retry_fixture("retry_on_401.json").await;
}

#[tokio::test]
async fn retries_next_auth_after_403_fixture() {
    run_auth_retry_fixture("retry_on_403.json").await;
}

#[tokio::test]
async fn retries_next_auth_after_usage_limit_fixture() {
    run_auth_retry_fixture("retry_on_429_usage_limit.json").await;
}

async fn run_auth_retry_fixture(name: &str) {
    let fixture = load_auth_retry_fixture(name);
    let seen_auths = Arc::new(Mutex::new(Vec::<String>::new()));
    let upstream_url = spawn_auth_retry_upstream(&fixture, Arc::clone(&seen_auths)).await;
    let auth_state = AuthStateOverlay::new();
    let app = router_with_snapshot_client_and_usage_queue(
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
        None,
        UsageQueue::new(),
        auth_state.clone(),
    );

    let response = app
        .clone()
        .oneshot(build_retry_request())
        .await
        .expect("call app");
    let status = response.status();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    assert_eq!(
        status,
        StatusCode::OK,
        "unexpected response body: {}",
        String::from_utf8_lossy(&body)
    );
    let payload: Value = serde_json::from_slice(&body).expect("parse body");
    assert_eq!(payload["auth"], fixture.expected_auth);

    let first_seen = seen_auths.lock().expect("seen auths").clone();
    assert_eq!(
        first_seen,
        vec![
            "Bearer codex-token-a".to_string(),
            "Bearer codex-token-b".to_string()
        ]
    );

    let now = time::OffsetDateTime::now_utc();
    let auth_key = AuthKey::new("auth-codex-a-index").expect("auth key");
    let auth_blocked_until = auth_state.auth_blocked_until(&auth_key, now);
    let model_blocked_until =
        cliproxy_data_plane::auth_state::ModelKey::new(auth_key.clone(), "gpt-5-codex")
            .and_then(|model_key| auth_state.model_blocked_until(&model_key, now));
    match fixture.cooldown_scope.as_str() {
        "auth" => {
            assert!(auth_blocked_until.is_some());
            assert!(model_blocked_until.is_none());
        }
        "model" => {
            assert!(auth_blocked_until.is_none());
            assert!(model_blocked_until.is_some());
        }
        other => panic!("unsupported cooldown_scope fixture value: {other}"),
    }

    if fixture.expect_second_request_skips_primary {
        seen_auths.lock().expect("seen auths").clear();
        let second = app.oneshot(build_retry_request()).await.expect("call app");
        assert_eq!(second.status(), StatusCode::OK);
        let second_seen = seen_auths.lock().expect("seen auths").clone();
        assert_eq!(second_seen, vec!["Bearer codex-token-b".to_string()]);
    }
}

fn load_auth_retry_fixture(name: &str) -> AuthRetryFixture {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root");
    let raw = std::fs::read_to_string(root.join("testdata/contract/auth").join(name))
        .expect("read auth retry fixture");
    serde_json::from_str(&raw).expect("parse auth retry fixture")
}

fn build_retry_request() -> Request<AxumBody> {
    Request::builder()
        .method("POST")
        .uri("/v1/responses")
        .header("content-type", "application/json")
        .body(AxumBody::from(
            json!({
                "model":"codex-latest",
                "stream":false,
                "input":"retry please"
            })
            .to_string(),
        ))
        .expect("build request")
}

async fn spawn_auth_retry_upstream(
    fixture: &AuthRetryFixture,
    seen_auths: Arc<Mutex<Vec<String>>>,
) -> String {
    use tokio::net::TcpListener;

    let first_status = fixture.first_status;
    let first_body = fixture.first_body.clone();
    let app = AxumRouter::new().route(
        "/responses",
        axum_post(move |headers: HeaderMap, request: Request<AxumBody>| {
            let seen_auths = Arc::clone(&seen_auths);
            let first_body = first_body.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                seen_auths.lock().expect("seen auths").push(auth.clone());

                if auth == "Bearer codex-token-a" {
                    return (
                        StatusCode::from_u16(first_status).expect("status"),
                        [("content-type", "application/json")],
                        AxumBody::from(first_body.to_string()),
                    )
                        .into_response();
                }

                let body = request
                    .into_body()
                    .collect()
                    .await
                    .expect("collect body")
                    .to_bytes();
                let payload: Value = serde_json::from_slice(&body).expect("parse payload");
                let stream = payload
                    .get("stream")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false);
                if stream {
                    (
                        StatusCode::OK,
                        [("content-type", "text/event-stream; charset=utf-8")],
                        AxumBody::from(format!(
                            "event: response.completed\ndata: {{\"type\":\"response.completed\",\"response\":{{\"id\":\"resp-retry-1\",\"object\":\"response\",\"status\":\"completed\",\"auth\":\"{}\",\"model\":{}}}}}\n\n",
                            auth,
                            payload["model"]
                        )),
                    )
                        .into_response()
                } else {
                    AxumJson(json!({
                        "object": "response",
                        "status": "completed",
                        "auth": auth,
                        "model": payload["model"]
                    }))
                    .into_response()
                }
            }
        }),
    );

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve upstream");
    });
    format!("http://{}", addr)
}
