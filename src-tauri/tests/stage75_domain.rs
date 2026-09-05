use inxaiot_desk_buddy_lib::domain::aio::release_profile::{
    ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues,
};
use inxaiot_desk_buddy_lib::domain::common::project::{PlatformLoginRequest, ProjectInput};

fn release_draft() -> ReleaseProfileDraft {
    ReleaseProfileDraft {
        values: ReleaseProfileValues {
            env_template:
                "PLATFORM_HOST={{platform.host}}\nDEVICE_EDGE_IMAGE={{image.device-edge}}\n".into(),
            compose_template: "services:\n  device-edge:\n    image: ${DEVICE_EDGE_IMAGE}\n".into(),
            host_info_template: r#"{"mac":"{{ node.mac }}","ip":"{{ node.ip }}","hostname":"{{ node.name }}","authKey":"{{ authKey }}"}"#.into(),
            platform_host: "192.168.3.6".into(),
            platform_api_port: 8055,
            platform_mqtt_host: "192.168.3.6".into(),
            platform_mqtt_port: 1883,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy".into(),
        },
        credentials: ReleaseProfileCredentials {
            platform_auth_key: "auth-key".into(),
            platform_mqtt_user: "platform".into(),
            platform_mqtt_password: "platform-password".into(),
            aio_mqtt_user: "aio".into(),
            aio_mqtt_password: "aio-password".into(),
            ssh_user: "root".into(),
            ssh_password: Some("ssh-password".into()),
            ssh_private_key: None,
        },
        expected_version: None,
    }
}

#[test]
fn release_profile_validation_blocks_unknown_templates_and_unsafe_remote_roots() {
    let valid = release_draft().validate().expect("valid profile");
    assert!(valid.valid);
    assert_eq!(valid.recognized_placeholder_count, 6);

    let mut unknown = release_draft();
    unknown
        .values
        .env_template
        .push_str("BAD={{unknown.value}}\n");
    assert!(unknown.validate().is_err());

    let mut unsafe_root = release_draft();
    unsafe_root.values.aio_data_root = "/".into();
    assert!(unsafe_root.validate().is_err());
}

#[test]
fn release_profile_reports_all_invalid_fields_without_credential_values() {
    let mut draft = release_draft();
    draft.credentials.platform_auth_key.clear();
    draft.credentials.platform_mqtt_password = "ab".into();
    draft.values.platform_api_port = 0;
    draft.values.aio_data_root = "/".into();
    let error = draft.validate().unwrap_err();
    let dto = inxaiot_desk_buddy_lib::interface::error::CommandErrorDto::from(error);
    assert_eq!(dto.code, "CONFIG_VALIDATION_FAILED");
    assert_eq!(dto.field_errors.len(), 4);
    for field in [
        "credentials.platformAuthKey",
        "credentials.platformMqttPassword",
        "values.platformApiPort",
        "values.aioDataRoot",
    ] {
        assert!(dto.field_errors.contains_key(field), "{field}");
    }
    let json = serde_json::to_value(dto).unwrap();
    assert!(json["fieldErrors"]["credentials.platformAuthKey"].is_string());
    assert!(!json.to_string().contains("ssh-password"));
}

#[test]
fn release_profile_associates_alternative_ssh_credentials_and_template_errors() {
    use inxaiot_desk_buddy_lib::core::error::AppError;
    let mut draft = release_draft();
    draft.credentials.ssh_password = None;
    let AppError::InvalidFields(fields) = draft.validate().unwrap_err() else {
        panic!("应返回字段错误");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(
        fields["credentials.sshPassword"],
        fields["credentials.sshPrivateKey"]
    );
    draft.credentials.ssh_password = Some("ssh-password".into());
    assert!(draft.validate().is_ok());

    draft
        .values
        .env_template
        .push_str("BAD={{unknown.value}}\n");
    let AppError::InvalidFields(fields) = draft.validate().unwrap_err() else {
        panic!("应返回字段错误");
    };
    assert!(fields.contains_key("values.envTemplate"));
    assert!(!fields.contains_key("values.composeTemplate"));
    draft.values.env_template = release_draft().values.env_template;
    draft
        .values
        .compose_template
        .push_str("    user: ${MISSING_USER}\n");
    let AppError::InvalidFields(fields) = draft.validate().unwrap_err() else {
        panic!("应返回字段错误");
    };
    assert!(fields.contains_key("values.composeTemplate"));
}

#[test]
fn project_input_requires_safe_urls_and_database_names() {
    let mut input = ProjectInput {
        name: "Project".into(),
        platform_url: "http://platform.test:8055".into(),
        db_host: "database.test".into(),
        db_port: 3306,
        db_user: "workbench".into(),
        db_tls_enabled: false,
        db_password: Some("password".into()),
        business_db: "inxvision_iot_dev".into(),
        workbench_db: "inxaiot_desk_buddy".into(),
    };
    input.validate_for_create().expect("valid project");
    input.business_db = "inxvision-iot;drop".into();
    assert!(input.validate_for_create().is_err());

    input.business_db = "inxvision_iot_dev".into();
    input.db_password = Some("x".into());
    assert!(input.validate_for_create().is_err());

    assert!(
        PlatformLoginRequest {
            username: "user".into(),
            password: "x".into(),
            session_uuid: "session".into(),
            image_code: "code".into(),
        }
        .validate()
        .is_err()
    );
}

#[test]
fn deployment_snapshot_binds_project_plan_nodes_and_host_keys() {
    let node = WorkbenchNodeSnapshot {
        mac_normalized: "001122334455".into(),
        name: "AIO".into(),
        ip: "192.0.2.10".into(),
        building_id: Some("1".into()),
        region_id: None,
        addr_alias: None,
        floor: None,
        location: None,
        remark: None,
        platform_aio_id: Some("1".into()),
        management_state: "managed".into(),
        source: "test".into(),
        last_operation_id: None,
        version: 7,
    };
    let mut snapshot = DeploymentExecutionSnapshot {
        schema_version: DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION,
        local_project_id: "project-a".into(),
        checked_at: "123".into(),
        profile_version: 3,
        artifact_fingerprint: "a".repeat(64),
        plan: DeploymentPlanInput {
            mode: DeploymentMode::FullUpgrade,
            target_macs: vec![node.mac_normalized.clone()],
            image_files: vec![
                inxaiot_desk_buddy_lib::domain::aio::deployment::DeploymentImageInput {
                    service_name: "device-edge".into(),
                    file_path: "C:/device-edge.tar".into(),
                    image_tag: "device-edge:1".into(),
                },
            ],
            artifact_path: "C:/release".into(),
            artifact_name: "Release".into(),
            artifact_version: "1.0.0".into(),
            service_name: None,
            image_name: None,
            service_image_environment_variable: None,
            images: BTreeMap::new(),
            batch_size: 1,
            concurrency: 1,
        },
        targets: vec![DeploymentTargetSnapshot {
            ssh_host: node.ip.clone(),
            ssh_port: 22,
            host_key_algorithm: "ssh-ed25519".into(),
            host_key_fingerprint: "SHA256:test".into(),
            host_key_accepted_at: "123".into(),
            node,
        }],
    };
    snapshot.validate("project-a").expect("valid snapshot");
    assert!(snapshot.validate("project-b").is_err());
    snapshot.artifact_fingerprint = "bad".into();
    assert!(snapshot.validate("project-a").is_err());
}
use std::collections::BTreeMap;

use inxaiot_desk_buddy_lib::domain::aio::deployment::{DeploymentMode, DeploymentPlanInput};
use inxaiot_desk_buddy_lib::domain::aio::deployment_workflow::{
    DEPLOYMENT_SNAPSHOT_SCHEMA_VERSION, DeploymentExecutionSnapshot, DeploymentTargetSnapshot,
};
use inxaiot_desk_buddy_lib::domain::aio::inventory::WorkbenchNodeSnapshot;
