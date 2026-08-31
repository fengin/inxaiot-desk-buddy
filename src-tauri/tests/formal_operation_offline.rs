use inxaiot_desk_buddy_lib::formal::operation_repository::validate_summary_json;

#[test]
fn operation_summary_allows_only_non_sensitive_final_metadata() {
    assert!(
        validate_summary_json(&serde_json::json!({
            "releaseMode": "full_upgrade",
            "service": "device-edge",
            "releaseProfileVersion": 3,
            "classificationCounts": {"new": 2, "existing": 4}
        }))
        .is_ok()
    );
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
            validate_summary_json(&serde_json::json!({field: "must-not-persist"})).is_err(),
            "field should be rejected: {field}"
        );
    }
    assert!(
        validate_summary_json(&serde_json::json!({
            "safe": [{"nestedLocalPath": "D:/release"}]
        }))
        .is_err()
    );
}
