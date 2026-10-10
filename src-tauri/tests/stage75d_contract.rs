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
fn mysql_tls_is_project_optional_defaults_to_plaintext_and_uses_rustls_when_enabled() {
    let manifest = include_str!("../Cargo.toml");
    assert!(manifest.contains("runtime-tokio-rustls"));
    assert!(!manifest.contains("runtime-tokio-native-tls"));
    let source = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/infrastructure/stage75_adapter.rs"),
    )
    .expect("stage75 adapter source");
    assert!(!source.contains("INX_DESKTOP_E2E_ALLOW_PLAINTEXT_MYSQL"));
    assert!(source.contains("tls_mode: if input.db_tls_enabled"));
    assert!(source.contains("DatabaseTlsMode::Required"));
    assert!(source.contains("DatabaseTlsMode::Disabled"));
    assert!(!source.contains("DatabaseTlsMode::Preferred"));
}

#[test]
fn ssh_rsa_signing_uses_aws_lc_without_russh_rsa_feature_or_yanked_chacha20() {
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
    let signing = include_str!("../src/infrastructure/remote/private_key.rs");
    assert!(signing.contains("signature::RsaKeyPair"));
    assert!(signing.contains("signature::RSA_PKCS1_SHA256"));
    assert!(signing.contains("signature::RSA_PKCS1_SHA512"));
    assert!(!signing.contains("rsa::pkcs1v15::SigningKey"));
    assert!(!signing.contains("RSA_PKCS1_SHA1"));
}

#[test]
fn distribution_has_explicit_windows_macos_and_linux_entries() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap();
    let release = std::fs::read_to_string(root.join("scripts/release-internal.ps1")).unwrap();
    assert!(release.contains("pnpm build:windows"));
    assert!(release.contains("schemaVersion = 5"));
    assert!(release.contains("package = \"portable-exe\""));
    assert!(!release.contains("--bundles nsis"));
    assert!(!release.contains("-setup.exe"));
    let windows: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.release.conf.json")).unwrap();
    assert_eq!(windows["bundle"]["active"], false);
    let macos: serde_json::Value =
        serde_json::from_str(include_str!("../tauri.macos.conf.json")).unwrap();
    assert_eq!(macos["bundle"]["targets"], serde_json::json!(["app"]));
    assert_eq!(
        macos["bundle"]["icon"],
        serde_json::json!(["icons/icon.png", "icons/icon.icns"])
    );
    let package: serde_json::Value =
        serde_json::from_str(include_str!("../../package.json")).unwrap();
    assert_eq!(
        package["scripts"]["build:windows"],
        "node scripts/build-windows.mjs"
    );
    let windows_builder = std::fs::read_to_string(root.join("scripts/build-windows.mjs")).unwrap();
    let prepare_adb = windows_builder
        .find("scripts/package-screen-tools.mjs")
        .unwrap();
    let build_app = windows_builder
        .find("node_modules/@tauri-apps/cli/tauri.js")
        .unwrap();
    let verify_package = windows_builder.find("--verify-package").unwrap();
    assert!(prepare_adb < build_app && build_app < verify_package);
    assert!(windows_builder.contains("--embedded-output"));
    assert!(windows_builder.contains("INX_EMBEDDED_ADB:archive"));
    assert!(
        windows_builder
            .contains("'build','--no-bundle','--config','src-tauri/tauri.release.conf.json'")
    );
    assert!(windows_builder.contains("readFileSync(executable).includes(readFileSync(archive))"));
    assert!(windows_builder.contains("!checked.successful || !checked.embedded"));
    assert_eq!(
        package["scripts"]["build:linux"],
        "node scripts/build-linux.mjs"
    );
    assert!(root.join("scripts/build-linux.mjs").is_file());
    let main = std::fs::read_to_string(root.join("src-tauri/src/main.rs")).unwrap();
    assert!(main.contains("windows_subsystem = \"windows\""));
    let png = std::fs::read(root.join("src-tauri/icons/icon.png")).unwrap();
    assert_eq!(&png[..8], &[137, 80, 78, 71, 13, 10, 26, 10]);
    assert_eq!(u32::from_be_bytes(png[16..20].try_into().unwrap()), 256);
    assert_eq!(u32::from_be_bytes(png[20..24].try_into().unwrap()), 256);
    let icns = std::fs::read(root.join("src-tauri/icons/icon.icns")).unwrap();
    assert_eq!(&icns[..4], b"icns");
    let workflow =
        std::fs::read_to_string(root.join(".github/workflows/portable-release.yml")).unwrap();
    assert!(workflow.contains("- \"v*.*.*\""));
    assert!(workflow.contains("runner: macos-15-intel"));
    assert!(workflow.contains("runner: macos-15"));
    assert!(workflow.contains("runs-on: ubuntu-22.04"));
    assert!(workflow.contains("runs-on: windows-latest"));
    assert!(workflow.contains("run: pnpm build:windows"));
    assert!(workflow.contains("scripts/build-windows.test.mjs"));
    assert!(workflow.contains("APPLE_SIGNING_IDENTITY: \"-\""));
    assert!(workflow.contains("pnpm install --frozen-lockfile"));
    assert!(workflow.contains("softprops/action-gh-release@v3"));
    assert!(root.join("scripts/check-release-version.mjs").is_file());
}

#[test]
fn production_ssh_automatically_observes_keys_without_manual_confirmation_gates() {
    let preflight = include_str!("../src/infrastructure/stage75b_preflight_adapter.rs");
    let execution = include_str!("../src/infrastructure/deployment_service.rs");
    assert!(preflight.contains("ObservedConnector::new"));
    assert!(preflight.contains("host_key_changed"));
    assert!(!preflight.contains("confirm_host_key"));
    assert!(!preflight.contains("HostKeyPolicy::Require"));
    assert!(execution.contains("ObservedConnector::new"));
    assert!(execution.contains("SSH_HOST_KEY_CHANGED"));
    assert!(!execution.contains("HostKeyPolicy::Require"));
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
