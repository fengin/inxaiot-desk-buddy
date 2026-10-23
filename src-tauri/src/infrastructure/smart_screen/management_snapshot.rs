//! 配置页面专用聚合读取。仅明确不支持时降级；各分项沿用原协议与大小边界。
use super::device::{AdbDevice,DeviceCommandPort,XIAOXIN_PACKAGE};
use crate::core::error::{AppError,AppResult};
use base64::{Engine,engine::general_purpose::STANDARD};
use serde_json::{Value,json};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

pub(super) const METHOD:&str="readManagementSnapshot";
const MAX_JSON:usize=256*1024;
const MAX_ENVELOPE:usize=((MAX_JSON+2)/3)*4+64;
#[derive(Debug)]
pub(super) struct Sections {pub capabilities:Value,pub config:Value,pub home:Value,pub kiosk:Value}
#[derive(Debug)]
pub(super) enum Snapshot {Unsupported,Data(Sections)}

fn invalid()->AppError {AppError::Conflict("小新聚合读取结果格式无效或超出大小限制，请检查配套版本并重新读取；未回退旧读取".into())}
fn section(data:&Value,key:&str,limit:usize)->Value {
    let value=&data[key];
    if !value.is_object() || !matches!(value["status"].as_str(),Some("ok"|"failed")) {
        return json!({"status":"failed","code":"INVALID_RESPONSE"});
    }
    if serde_json::to_vec(value).map_or(true,|bytes|bytes.len()>limit) {
        return json!({"status":"failed","code":"RESPONSE_TOO_LARGE"});
    }
    value.clone()
}

pub(super) fn parse(text:&str)->AppResult<Snapshot> {
    if text.len()>MAX_ENVELOPE {return Err(invalid())}
    let encoded=text.strip_prefix("Result: Bundle[{result=").and_then(|s|s.strip_suffix("}]")).ok_or_else(invalid)?;
    let bytes=STANDARD.decode(encoded).map_err(|_|invalid())?;
    if bytes.len()>MAX_JSON {return Err(invalid())}
    let response:Value=serde_json::from_slice(&bytes).map_err(|_|invalid())?;
    if response["status"]=="failed" && response["code"]=="UNSUPPORTED" {return Ok(Snapshot::Unsupported)}
    if response["status"]=="failed" {
        return Err(AppError::Conflict(match response["code"].as_str() {
            Some("FORBIDDEN")=>"设备拒绝当前管理身份，不能读取配置；未回退旧读取",
            Some("RESPONSE_TOO_LARGE")=>"小新聚合读取结果超过大小限制；未回退旧读取",
            _=>"小新聚合读取未完成，请检查应用状态后重新读取；未回退旧读取",
        }.into()));
    }
    let data=&response["data"];
    if response["status"]!="ok" || data["snapshotVersion"]!=1 || data["packageName"]!=XIAOXIN_PACKAGE {return Err(invalid())}
    Ok(Snapshot::Data(Sections {capabilities:section(data,"capabilities",64*1024),config:section(data,"config",64*1024),
        home:section(data,"home",16*1024),kiosk:section(data,"kiosk",64*1024)}))
}

pub(super) async fn read<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str)->AppResult<Snapshot> {
    device.connect(ip,CancellationToken::new()).await?;
    let output=device.adb(ip,&["shell","content","call","--uri",super::app_config::URI,"--method",METHOD],
        Duration::from_secs(20),CancellationToken::new()).await?;
    if !output.success || !output.stderr.trim().is_empty() {
        return Err(AppError::Conflict("小新聚合读取连接或权限未确认；未回退旧读取".into()));
    }
    parse(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn envelope(value:Value)->String {format!("Result: Bundle[{{result={}}}]",STANDARD.encode(serde_json::to_vec(&value).unwrap()))}
    fn fixture()->Value {json!({"status":"ok","data":{"snapshotVersion":1,"packageName":XIAOXIN_PACKAGE,
        "capabilities":{"status":"ok","data":{}},"config":{"status":"ok","data":{}},
        "home":{"status":"ok","data":{}},"kiosk":{"status":"ok","data":{}}}})}
    #[test]
    fn only_explicit_top_level_unsupported_is_a_fallback_signal() {
        assert!(matches!(parse(&envelope(json!({"status":"failed","code":"UNSUPPORTED"}))).unwrap(),Snapshot::Unsupported));
        for bad in [json!({"status":"failed","code":"FORBIDDEN"}),json!({"status":"error","code":"UNSUPPORTED"}),
            json!({"status":"failed","code":"NOT_READY"}),json!({"status":"failed","code":"RESPONSE_TOO_LARGE"})] {assert!(parse(&envelope(bad)).is_err());}
        let mut data=fixture();data["data"]["home"]=json!({"status":"failed","code":"UNSUPPORTED"});
        assert!(matches!(parse(&envelope(data)).unwrap(),Snapshot::Data(_)));
    }
    #[test]
    fn snapshot_and_section_size_limits_do_not_change_the_legacy_parser() {
        let mut large=fixture();large["data"]["config"]["data"]["padding"]=json!("x".repeat(63*1024));
        large["data"]["capabilities"]["data"]["padding"]=json!("x".repeat(63*1024));
        let text=envelope(large.clone());assert!(parse(&text).is_ok());assert!(super::super::app_config::parse_response(&text).is_err());
        large["data"]["config"]["data"]["padding"]=json!("x".repeat(65*1024));
        let Snapshot::Data(sections)=parse(&envelope(large.clone())).unwrap() else {panic!()};
        assert_eq!(sections.config["code"],"RESPONSE_TOO_LARGE");assert_eq!(sections.capabilities["status"],"ok");
        let mut home=fixture();home["data"]["home"]["data"]["padding"]=json!("x".repeat(17*1024));
        let Snapshot::Data(sections)=parse(&envelope(home)).unwrap() else {panic!()};
        assert_eq!(sections.home["code"],"RESPONSE_TOO_LARGE");assert_eq!(sections.config["status"],"ok");
        large["data"]["extra"]=json!("x".repeat(256*1024));assert!(parse(&envelope(large)).is_err());
    }
    #[test]
    fn missing_segments_are_errors_while_bad_frames_are_rejected() {
        let mut missing=fixture();missing["data"].as_object_mut().unwrap().remove("kiosk");
        let Snapshot::Data(sections)=parse(&envelope(missing)).unwrap() else {panic!()};
        assert_eq!(sections.kiosk["code"],"INVALID_RESPONSE");assert_eq!(sections.config["status"],"ok");
        for bad in ["", "Result: Bundle[{result=%%%}]","Result: Bundle[{result=e30=}]","Could not find provider"] {assert!(parse(bad).is_err());}
        for (key,value) in [("snapshotVersion",json!(2)),("packageName",json!("other.app"))] {
            let mut data=fixture();data["data"][key]=value;assert!(parse(&envelope(data)).is_err());
        }
    }
}
