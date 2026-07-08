use axum::{
    body::Body as AxumBody,
    http::{Request, StatusCode},
};
use cliproxy_common_types::snapshot::RoutingStrategy;
use cliproxy_data_plane::http::router;
use http_body_util::BodyExt;
use serde::Deserialize;
use serde_json::Value;
use tower::ServiceExt;

use crate::common::{
    codex_oauth_auth, spawn_openai_upstream, test_runtime_with_auths, test_upstream,
};

#[derive(Debug, Deserialize)]
struct ResponsesGoldenFixture {
    request: Value,
    expected_response: Value,
}

#[tokio::test]
async fn non_stream_response_matches_golden_fixture() {
    let fixture = load_responses_fixture("non_stream_aggregates_codex_stream.json");
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
                .body(AxumBody::from(fixture.request.to_string()))
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
    let actual: Value = serde_json::from_slice(&body).expect("parse response body");

    assert_eq!(actual, fixture.expected_response);
}

fn load_responses_fixture(name: &str) -> ResponsesGoldenFixture {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root");
    let raw = std::fs::read_to_string(root.join("testdata/contract/responses").join(name))
        .expect("read responses golden fixture");
    serde_json::from_str(&raw).expect("parse responses golden fixture")
}
