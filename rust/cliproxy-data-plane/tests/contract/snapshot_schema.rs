use cliproxy_common_types::snapshot::RuntimeSnapshot;
use cliproxy_runtime_config_client::validate_snapshot;

#[test]
fn parses_go_exported_runtime_snapshot_golden() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let raw =
        std::fs::read_to_string(root.join("testdata/contract/runtime_snapshot.codex.golden.json"))
            .expect("read Go-exported snapshot golden");

    let snapshot: RuntimeSnapshot =
        serde_json::from_str(&raw).expect("parse Go-exported snapshot golden");

    validate_snapshot(&snapshot).expect("validate Go-exported snapshot golden");
}

#[test]
fn rejects_invalid_snapshot_missing_version_fixture() {
    let snapshot = load_invalid_snapshot("runtime_snapshot.invalid_missing_version.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("snapshot.version"));
}

#[test]
fn rejects_invalid_snapshot_missing_generated_at() {
    let snapshot = load_invalid_snapshot("runtime_snapshot.invalid_missing_generated_at.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("snapshot.generated_at"));
}

#[test]
fn rejects_invalid_snapshot_missing_source_instance_id() {
    let snapshot =
        load_invalid_snapshot("runtime_snapshot.invalid_missing_source_instance_id.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("snapshot.source_instance_id"));
}

#[test]
fn rejects_invalid_snapshot_codex_missing_access_token() {
    let snapshot =
        load_invalid_snapshot("runtime_snapshot.invalid_codex_missing_access_token.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("execution.codex.access_token"));
}

#[test]
fn rejects_invalid_snapshot_empty_model_alias_target() {
    let snapshot = load_invalid_snapshot("runtime_snapshot.invalid_empty_model_alias_target.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("model_aliases"));
}

#[test]
fn rejects_invalid_snapshot_provider_missing_model() {
    let snapshot = load_invalid_snapshot("runtime_snapshot.invalid_provider_missing_model.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("models"));
}

#[test]
fn rejects_invalid_snapshot_empty_provider_key() {
    let snapshot = load_invalid_snapshot("runtime_snapshot.invalid_empty_provider_key.json");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");
    assert!(err.to_string().contains("providers"));
}

fn load_invalid_snapshot(name: &str) -> RuntimeSnapshot {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let raw = std::fs::read_to_string(root.join("testdata/contract").join(name))
        .expect("read invalid snapshot fixture");
    serde_json::from_str(&raw).expect("parse invalid snapshot fixture")
}
