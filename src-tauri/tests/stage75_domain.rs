use inxaiot_desk_buddy_lib::domain::aio::release_profile::{
    ReleaseProfileCredentials, ReleaseProfileDraft, ReleaseProfileValues,
};
use inxaiot_desk_buddy_lib::domain::common::project::ProjectInput;

fn release_draft() -> ReleaseProfileDraft {
    ReleaseProfileDraft {
        values: ReleaseProfileValues {
            env_template:
                "PLATFORM_HOST={{platform.host}}\nDEVICE_EDGE_IMAGE={{image.device-edge}}\n".into(),
            compose_template: "services:\n  device-edge:\n    image: ${DEVICE_EDGE_IMAGE}\n".into(),
            platform_host: "192.168.3.6".into(),
            platform_api_port: 8055,
            platform_mqtt_host: "192.168.3.6".into(),
            platform_mqtt_port: 1883,
            ssh_port: 22,
            ssh_timeout_seconds: 15,
            aio_data_root: "/opt/data".into(),
            aio_deploy_root: "/opt/data/deploy/inxvision-edge".into(),
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
    assert_eq!(valid.recognized_placeholder_count, 2);

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
fn project_input_requires_safe_urls_and_database_names() {
    let mut input = ProjectInput {
        name: "Project".into(),
        platform_url: "http://platform.test:8055".into(),
        db_host: "database.test".into(),
        db_port: 3306,
        db_user: "workbench".into(),
        db_password: Some("password".into()),
        business_db: "inxvision_iot_dev".into(),
        workbench_db: "inxaiot_desk_buddy".into(),
    };
    input.validate_for_create().expect("valid project");
    input.business_db = "inxvision-iot;drop".into();
    assert!(input.validate_for_create().is_err());
}
