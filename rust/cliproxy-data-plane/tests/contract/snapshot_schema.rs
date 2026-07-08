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
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("workspace root");
    let raw = std::fs::read_to_string(
        root.join("testdata/contract/runtime_snapshot.invalid_missing_version.json"),
    )
    .expect("read invalid snapshot fixture");

    let snapshot: RuntimeSnapshot =
        serde_json::from_str(&raw).expect("parse invalid snapshot fixture");
    let err = validate_snapshot(&snapshot).expect_err("fixture should fail validation");

    assert!(err.to_string().contains("snapshot.version"));
}
