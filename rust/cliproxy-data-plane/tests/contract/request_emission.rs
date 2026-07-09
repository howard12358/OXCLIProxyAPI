use std::sync::{Arc, Mutex};

use axum::{
    Router as AxumRouter,
    body::Body as AxumBody,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::post as axum_post,
};
use cliproxy_data_plane::http::router;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use crate::common::{codex_oauth_auth, openai_upstream, test_runtime_with_auths};

#[tokio::test]
async fn codex_request_emission_matches_golden_fixtures() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("repo root")
        .join("testdata/contract/responses/request_emission");

    let mut entries: Vec<_> = std::fs::read_dir(&root)
        .expect("read request_emission dir")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .map(|ext| ext == "json")
                .unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect();
    entries.sort();

    let mut names: Vec<String> = Vec::new();
    for path in &entries {
        let file_name = path.file_stem().unwrap().to_string_lossy();
        if let Some((name, suffix)) = file_name.rsplit_once('.')
            && suffix == "request"
            && !names.contains(&name.to_string())
        {
            names.push(name.to_string());
        }
    }

    for name in names {
        let request_path = root.join(format!("{name}.request.json"));
        let expected_path = root.join(format!("{name}.expected.json"));

        let request: Value = serde_json::from_str(
            &std::fs::read_to_string(&request_path).expect("read request fixture"),
        )
        .expect("parse request fixture");
        let expected: Value = serde_json::from_str(
            &std::fs::read_to_string(&expected_path).expect("read expected fixture"),
        )
        .expect("parse expected fixture");

        let captured = Arc::new(Mutex::new(None));
        let upstream_url = spawn_capturing_codex_upstream(Arc::clone(&captured)).await;
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
                    .body(AxumBody::from(request.to_string()))
                    .expect("build request"),
            )
            .await
            .expect("call app");

        assert_eq!(
            response.status(),
            StatusCode::OK,
            "fixture {name} should return 200"
        );

        let guard = captured.lock().expect("lock captured body");
        let actual = guard
            .as_ref()
            .expect("upstream body should be captured")
            .clone();
        drop(guard);

        let normalized_actual = normalize_json(&actual);
        let normalized_expected = normalize_json(&expected);

        assert_eq!(
            normalized_actual, normalized_expected,
            "fixture {name} upstream request mismatch"
        );
    }
}

async fn spawn_capturing_codex_upstream(captured: Arc<Mutex<Option<Value>>>) -> String {
    use tokio::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("upstream addr");

    let completed_response = concat!(
        "event: response.completed\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp-emission-1\",\"object\":\"response\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}]}}\n\n"
    )
    .to_string();

    let app = AxumRouter::new().route(
        "/responses",
        axum_post(move |request: Request<AxumBody>| {
            let captured = Arc::clone(&captured);
            let completed_response = completed_response.clone();
            async move {
                let body = request
                    .into_body()
                    .collect()
                    .await
                    .expect("collect body")
                    .to_bytes();
                let payload: Value = serde_json::from_slice(&body).expect("parse upstream body");
                *captured.lock().expect("lock captured body") = Some(payload);

                (
                    StatusCode::OK,
                    [("content-type", "text/event-stream; charset=utf-8")],
                    AxumBody::from(completed_response),
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

fn normalize_json(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect();
            entries.sort_by_key(|(a, _)| *a);
            Value::Object(
                entries
                    .into_iter()
                    .map(|(k, v)| (k.clone(), normalize_json(v)))
                    .collect(),
            )
        }
        Value::Array(arr) => Value::Array(arr.iter().map(normalize_json).collect()),
        other => other.clone(),
    }
}
