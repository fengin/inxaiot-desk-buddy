use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseRenderContext {
    pub release_version: String,
    pub platform_host: String,
    pub platform_api_port: u16,
    pub platform_username: String,
    pub platform_password: String,
    pub platform_mqtt_host: String,
    pub platform_mqtt_port: u16,
    pub platform_mqtt_user: String,
    pub platform_mqtt_password: String,
    pub local_mqtt_user: String,
    pub local_mqtt_password: String,
    pub auth_key: String,
    pub node_name: String,
    pub node_ip: String,
    pub node_mac: String,
    #[serde(default)]
    pub node_building_id: String,
    #[serde(default)]
    pub node_region_id: String,
    #[serde(default)]
    pub node_addr_alias: String,
    #[serde(default)]
    pub node_floor: String,
    #[serde(default)]
    pub node_location: String,
    #[serde(default)]
    pub node_remark: String,
    #[serde(default)]
    pub images: BTreeMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedReleaseFiles {
    pub env: String,
    pub host_info_json: String,
    pub compose_preview: String,
    /// 部署文件保留四个可单服升级的镜像引用；不暴露额外前端字段。
    #[serde(skip)]
    pub compose_runtime: String,
}
