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
