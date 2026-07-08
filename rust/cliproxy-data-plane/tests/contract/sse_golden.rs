use axum::{
    Router as AxumRouter,
    body::Body as AxumBody,
    http::{HeaderMap, Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_data_plane::http::router;
use http_body_util::BodyExt;
use serde::Deserialize;
use tower::ServiceExt;

use crate::common::{openai_upstream, test_runtime};

#[derive(Debug, Deserialize)]
struct SseGoldenFixture {
    upstream_body: String,
    expected_body: String,
}

#[tokio::test]
async fn stream_repairs_completed_output_against_golden_fixture() {
    let fixture = load_sse_fixture("stream_repairs_completed_output.json");
    let upstream_url = spawn_sse_upstream(&fixture.upstream_body).await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(AxumBody::from(
                    serde_json::json!({"model":"codex-latest","stream":true,"input":"hello"})
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
    let actual = String::from_utf8(body.to_vec()).expect("valid utf8");

    assert_eq!(actual.trim(), fixture.expected_body.trim());
}

#[tokio::test]
async fn stream_preserves_done_and_non_json_frames_against_golden_fixture() {
    let fixture = load_sse_fixture("stream_preserves_done_and_non_json.json");
    let upstream_url = spawn_sse_upstream(&fixture.upstream_body).await;
    let app = router(test_runtime(true), openai_upstream(upstream_url));

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/responses")
                .header("content-type", "application/json")
                .body(AxumBody::from(
                    serde_json::json!({"model":"codex-latest","stream":true,"input":"hello"})
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
    let actual = String::from_utf8(body.to_vec()).expect("valid utf8");

    assert_eq!(actual.trim(), fixture.expected_body.trim());
}

fn load_sse_fixture(name: &str) -> SseGoldenFixture {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root");
    let raw = std::fs::read_to_string(root.join("testdata/contract/sse").join(name))
        .expect("read SSE golden fixture");
    serde_json::from_str(&raw).expect("parse SSE golden fixture")
}

async fn spawn_sse_upstream(body: &str) -> String {
    use tokio::net::TcpListener;
    let response_body = body.to_string();

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");
    let app = AxumRouter::new().route(
        "/responses",
        axum_post(move |_headers: HeaderMap, _request: Request<AxumBody>| {
            let response_body = response_body.clone();
            async move {
                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream; charset=utf-8")],
                    AxumBody::from(response_body),
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
