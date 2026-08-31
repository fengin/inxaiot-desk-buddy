use inxaiot_desk_buddy_lib::core::error::AppError;
use inxaiot_desk_buddy_lib::infrastructure::logging::redactor::SensitiveValueRedactor;
use inxaiot_desk_buddy_lib::interface::error::CommandErrorDto;

#[test]
fn command_error_dto_and_error_log_summary_use_registered_project_secrets() {
    let secret = format!("stage75d-secret-{}", uuid::Uuid::now_v7());
    SensitiveValueRedactor::production()
        .register_values([secret.clone()])
        .expect("register");
    let dto = CommandErrorDto::from(AppError::InvalidConfig(format!("连接参数包含{secret}")));
    let serialized = serde_json::to_string(&dto).expect("dto");
    assert!(!serialized.contains(&secret));
    assert!(serialized.contains("[REDACTED]"));
}

#[test]
fn production_logs_do_not_emit_raw_errors_or_untrusted_response_text() {
    let source_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let forbidden = [
        "error = ?error",
        "error = %error",
        "message = %response.message",
        "columns = ?unknown",
    ];
    let mut violations = Vec::new();
    for entry in walkdir::WalkDir::new(source_root) {
        let entry = entry.expect("walk production source");
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("rs")
        {
            continue;
        }
        let source = std::fs::read_to_string(entry.path()).expect("read production source");
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{}: {pattern}", entry.path().display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "生产日志出现原始错误或不受信任原文：{}",
        violations.join("；")
    );
}

#[test]
fn ssh_dependency_contract_disables_rsa_and_rejects_yanked_chacha20() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains(
        "russh = { version = \"0.63.1\", default-features = false, features = [\"aws-lc-rs\", \"flate2\"] }"
    ));
    let lock = include_str!("../Cargo.lock");
    let chacha = lock
        .split("[[package]]")
        .find(|package| package.contains("name = \"chacha20\""))
        .expect("chacha20 lock entry");
    assert!(chacha.contains("version = \"0.10.2\""));
    assert!(!lock.contains("version = \"0.10.0-rc.18\""));
}

#[test]
fn application_layer_has_no_concrete_infrastructure_or_sql_dependencies() {
    let application_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("application");
    let forbidden = ["crate::formal::", "crate::infrastructure::", "sqlx::"];
    let mut violations = Vec::new();
    for entry in walkdir::WalkDir::new(application_root) {
        let entry = entry.expect("walk application source");
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|value| value.to_str()) != Some("rs")
        {
            continue;
        }
        let source = std::fs::read_to_string(entry.path()).expect("read application source");
        for pattern in forbidden {
            if source.contains(pattern) {
                violations.push(format!("{}: {pattern}", entry.path().display()));
            }
        }
    }
    assert!(
        violations.is_empty(),
        "Application层出现具体基础设施依赖：{}",
        violations.join("；")
    );
}
