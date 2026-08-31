#[path = "../src/formal/error.rs"]
mod error;
#[path = "../src/formal/operation_repository_v2.rs"]
mod operation_repository;

use operation_repository::validate_summary_json_strict;

#[test]
fn strict_summary_filter_covers_common_naming_styles() {
    assert!(validate_summary_json_strict(&serde_json::json!({
        "releaseMode": "full_upgrade",
        "service": "device-edge",
        "releaseProfileVersion": 3,
        "classification-counts": {"new": 2, "existing": 4}
    }))
    .is_ok());

    for field in [
        "password",
        "ssh_private_key",
        "sshPrivateKey",
        "ssh-private-key",
        "platform_auth_key",
        "platformAuthKey",
        "platform-auth-key",
        "access_token",
        "accessToken",
        "local_path",
        "localPath",
        "progress",
        "taskSteps",
        "stdout",
        "stderr",
        "completeLog",
    ] {
        assert!(
            validate_summary_json_strict(&serde_json::json!({field: "must-not-persist"}))
                .is_err(),
            "field should be rejected: {field}"
        );
    }
    assert!(
        validate_summary_json_strict(&serde_json::json!({
            "safe": [{"nestedLocalPath": "D:/release"}]
        }))
        .is_err()
    );
}

