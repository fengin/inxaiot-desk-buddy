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
