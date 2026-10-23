//! 将小新自身的单一普通任务退出后交还系统 HOME；不迁移任务、不触碰其他应用。
use super::{device::{AdbDevice, DeviceCommandPort, XIAOXIN_PACKAGE}, home};
use crate::core::error::{AppError, AppResult};
use serde::Serialize;
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(rename_all="camelCase")]
pub struct ActivationPlan { pub kind: String, pub task_id: Option<u32> }

fn uncertain() -> AppError { AppError::Conflict("小新当前任务身份或类型无法唯一确认，未退出任务；请重新读取".into()) }
fn main(component: &str) -> bool { component == home::COMPONENT || component == "chat.xiaoxin.app/.MainActivity" }

/// 使用现有完整转储校验，再逐个显示屏/栈/任务绑定活动；不能借用另一任务中的 Main。
pub fn parse_plan(text: &str) -> AppResult<ActivationPlan> {
    home::parse_runtime(text)?;
    let lines=text.lines().map(|line|line.trim_end_matches('\r')).collect::<Vec<_>>();
    let display_re=regex::Regex::new(r"^Display #(\d+) \(activities from top to bottom\):$").unwrap();
    let stack_re=regex::Regex::new(r"^  Stack #(\d+):(?: type=([a-z]+) mode=[a-z-]+)?$").unwrap();
    let task_re=regex::Regex::new(r"^    Task id #(\d+)$").unwrap();
    let record_re=regex::Regex::new(r"^    \* TaskRecord\{[^\s]+ #(\d+) .* U=(\d+) StackId=(\d+) sz=(\d+)\}$").unwrap();
    let hist_re=regex::Regex::new(r"^      \* Hist #\d+: ActivityRecord\{([^\s]+) u(\d+) ([^\s]+) t(\d+)[^}]*\}$").unwrap();
    let own_record_re=regex::Regex::new(r"\b(?:A=|I=)(?:\d+:)?chat\.xiaoxin\.app(?:/|[ }\s])").unwrap();
    let mut display=None; let mut stack=None; let mut modern=false; let mut stack_kind=String::new();
    let mut own=Vec::new(); let mut index=0;
    while index<lines.len() {
        let line=lines[index];
        if let Some(c)=display_re.captures(line) { display=c[1].parse::<u32>().ok(); stack=None; }
        if let Some(c)=stack_re.captures(line) { stack=c[1].parse::<u32>().ok(); modern=c.get(2).is_some(); stack_kind=c.get(2).map(|v|v.as_str()).unwrap_or("").into(); }
        let Some(task)=task_re.captures(line) else { index+=1;continue };
        let end=(index+1..lines.len()).find(|next|task_re.is_match(lines[*next]) || lines[*next].starts_with("  Stack #")
            || lines[*next].starts_with("Display #") || lines[*next].starts_with("    Running activities")).unwrap_or(lines.len());
        let block=&lines[index+1..end];
        let histories=block.iter().enumerate().filter_map(|(i,line)|hist_re.captures(line).map(|c|(i,c))).collect::<Vec<_>>();
        let own_hist=histories.iter().filter(|(_,c)|c[3].starts_with(&format!("{XIAOXIN_PACKAGE}/"))).collect::<Vec<_>>();
        let own_record=block.iter().filter(|line|line.starts_with("    * TaskRecord{")).any(|line|own_record_re.is_match(line));
        if !own_hist.is_empty() || own_record {
            let record=block.iter().find_map(|line|record_re.captures(line)).ok_or_else(uncertain)?;
            let task_id=task[1].parse::<u32>().map_err(|_|uncertain())?;
            if display!=Some(0) || histories.len()!=1 || own_hist.len()!=1 || record[1].parse::<u32>().ok()!=Some(task_id)
                || &record[2]!="0" || record[3].parse::<u32>().ok()!=stack || &record[4]!="1" { return Err(uncertain()) }
            let (hist_index,hist)=own_hist[0];
            if &hist[2]!="0" || hist[4].parse::<u32>().ok()!=Some(task_id) || !main(&hist[3]) { return Err(uncertain()) }
            let header=block[..*hist_index].join("\n");
            let details=&block[*hist_index+1..];
            let task_kind=if header.split_whitespace().any(|token|token==if modern {"activityType=2"} else {"taskType=1"}) {"home"}
                else if header.split_whitespace().any(|token|token==if modern {"activityType=1"} else {"taskType=0"}) {"standard"} else {return Err(uncertain())};
            let activity_kind=if modern {task_kind} else if task_kind=="home" {"HOME_ACTIVITY_TYPE"} else {"APPLICATION_ACTIVITY_TYPE"};
            if (modern && stack_kind!=task_kind) || !details.iter().any(|line|line.trim()==format!("mActivityType={activity_kind}")) {return Err(uncertain())}
            own.push(ActivationPlan {kind:task_kind.into(),task_id:Some(task_id)});
        }
        index=end;
    }
    match own.len() {0=>Ok(ActivationPlan {kind:"absent".into(),task_id:None}),1=>Ok(own.remove(0)),_=>Err(uncertain())}
}

pub async fn query<P: DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,cancel:CancellationToken)->AppResult<ActivationPlan> {
    let text=device.shell(ip,&["dumpsys","activity","activities"],cancel).await?;
    parse_plan(&text)
}

pub async fn require_capability<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str)->AppResult<()> {
    let response=super::app_config::call(device,ip,"getCapabilities",None).await?;
    let cap=&response["data"]["homeManagement"];
    if response["status"]!="ok" || response["data"]["packageName"]!=XIAOXIN_PACKAGE || response["data"]["protocolVersion"]!=1
        || cap["protocolVersion"]!=1 || !cap["methods"].as_array().is_some_and(|methods|methods.iter().any(|v|v=="prepareHomeActivation")) {
        return Err(AppError::Conflict("当前小新不支持普通任务转为系统主屏，请升级配套应用后重新读取".into()));
    }
    Ok(())
}

pub async fn prepare<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,task_id:u32,request_id:&str)->AppResult<()> {
    let response=home::call_with_payload(device,ip,"prepareHomeActivation",Some(&json!({"protocolVersion":1,"requestId":request_id,"expectedTaskId":task_id}))).await?;
    let data=&response["data"];
    if response["status"]!="prepared" || response["requestId"]!=request_id || data["protocolVersion"]!=1
        || data["packageName"]!=XIAOXIN_PACKAGE || data["removedTaskId"]!=task_id || data["removalRequested"]!=true {
        return Err(AppError::Conflict(match response["code"].as_str() {
            Some("ACTIVITY_NOT_READY")=>"小新当前不在有效前台，未退出普通任务；请保持小新前台后重新执行明确激活",
            Some("KIOSK_ENABLED")=>"请先明确关闭应用锁定，再转换系统主屏",
            Some("TASK_CHANGED")=>"小新任务已变化，未继续转换系统主屏，请重新读取",
            _=>"退出小新普通任务的结果未确认，请只读核实；不会自动重复退出任务",
        }.into()));
    }
    Ok(())
}

pub fn task_removed(text:&str,task_id:u32)->AppResult<bool> {
    home::parse_runtime(text)?;
    // 检查所有任务编号，不能只看当前HOME编号而漏掉仍在后台的原普通任务。
    let tasks=regex::Regex::new(r"(?m)^    Task id #(\d+)\r?$").unwrap();
    Ok(!tasks.captures_iter(text).any(|c|c[1].parse::<u32>().ok()==Some(task_id)))
}

pub async fn query_removed<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,task_id:u32,cancel:CancellationToken)->AppResult<bool> {
    task_removed(&device.shell(ip,&["dumpsys","activity","activities"],cancel).await?,task_id)
}

pub async fn verify_preparation<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,evidence:&mut serde_json::Value)->AppResult<()> {
    let Some(task_id)=evidence["prepareTaskId"].as_u64().and_then(|id|u32::try_from(id).ok()) else {return Ok(())};
    evidence["taskRemovalReadback"]=json!("unknown");
    let removed=query_removed(device,ip,task_id,CancellationToken::new()).await?;
    evidence["taskRemovalReadback"]=json!(if removed {"succeeded"} else {"different"});
    if removed {evidence["preparation"]=json!("verified")}
    Ok(())
}

pub async fn wait_removed<P:DeviceCommandPort>(device:&AdbDevice<P>,ip:&str,task_id:u32,limit:Duration,cancel:CancellationToken)->AppResult<bool> {
    let deadline=tokio::time::Instant::now()+limit;
    loop {
        if cancel.is_cancelled() {return Err(AppError::Cancelled)}
        let remaining=deadline.saturating_duration_since(tokio::time::Instant::now());
        let removed=tokio::time::timeout(remaining,query_removed(device,ip,task_id,cancel.clone())).await
            .map_err(|_|AppError::Conflict("等待小新原任务退出超时，请只读核实".into()))??;
        if removed {return Ok(true)}
        if tokio::time::Instant::now()>=deadline {return Ok(false)}
        tokio::time::sleep(Duration::from_millis(250).min(deadline.saturating_duration_since(tokio::time::Instant::now()))).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::device::{AndroidTools,ProcessOutput};
    use base64::{Engine,engine::general_purpose::STANDARD};
    use std::{collections::VecDeque,path::Path,sync::{Arc,Mutex}};
    const ACTIVE:&str=include_str!("../../../tests/fixtures/home_runtime/android10_after_activation.txt");
    const LEGACY:&str=include_str!("../../../tests/fixtures/home_runtime/android81_real_home_awake.txt");
    const REAL_STANDARD_10:&str=include_str!("../../../tests/fixtures/home_runtime/android10_real_standard_task88.txt");
    const REAL_STANDARD_81:&str=include_str!("../../../tests/fixtures/home_runtime/android81_real_standard_task134.txt");
    fn ordinary(text:&str)->String {text.replace("type=home","type=standard").replace("activityType=2","activityType=1")
        .replace("mActivityType=home","mActivityType=standard").replace("taskType=1","taskType=0").replace("mActivityType=HOME_ACTIVITY_TYPE","mActivityType=APPLICATION_ACTIVITY_TYPE")}
    #[test]
    fn identifies_only_single_default_user_main_tasks_without_weakening_home() {
        for fixture in [ACTIVE,LEGACY] {
            assert_eq!(parse_plan(fixture).unwrap().kind,"home");
            let standard=ordinary(fixture);
            assert_eq!(parse_plan(&standard).unwrap().kind,"standard");
            assert!(!home::parse_runtime(&standard).unwrap().verified());
            for bad in [standard.replace(" u0 "," u10 "),standard.replace("sz=1}","sz=2}"),standard.replace("/.MainActivity","/.OtherActivity")] {
                assert!(parse_plan(&bad).is_err());
            }
        }
    }
    #[test]
    fn absent_is_only_accepted_after_complete_system_format() {
        let other=ordinary(ACTIVE).replace("chat.xiaoxin.app","other.app");
        assert_eq!(parse_plan(&other).unwrap().kind,"absent");
        assert!(parse_plan("").is_err());
        assert!(parse_plan(&ordinary(ACTIVE).replace("mActivityType=standard","mActivityType=unknown")).is_err());
    }
    #[test]
    fn actual_standard_tasks_are_recognized_but_never_count_as_home() {
        for (text,id,active_home) in [(REAL_STANDARD_10,88,ACTIVE),(REAL_STANDARD_81,134,LEGACY)] {
            assert_eq!(parse_plan(text).unwrap(),ActivationPlan {kind:"standard".into(),task_id:Some(id)});
            assert!(!home::parse_runtime(text).unwrap().verified());
            assert!(!task_removed(text,id).unwrap());
            assert!(home::parse_runtime(active_home).unwrap().verified());
            assert!(task_removed(active_home,id).unwrap());
            let repeated=text.replace(&format!("    Task id #{id}"),&format!("    Task id #{id}\n    Task id #999"));
            assert!(parse_plan(&repeated).is_err());
        }
    }
    #[test]
    fn a_new_foreground_home_does_not_hide_an_old_background_standard_task() {
        let start=LEGACY.find("  Stack #0:").unwrap();
        let end=LEGACY.find("  Stack #1:").unwrap();
        let combined=REAL_STANDARD_81.replace("  Stack #0:\n  isSleeping=false\n",&LEGACY[start..end])
            .replace("mFocusedStack=ActivityStack{d52a2b7 stackId=1","mFocusedStack=ActivityStack{d52a2b7 stackId=0");
        assert!(home::parse_runtime(&combined).unwrap().verified());
        assert!(!task_removed(&combined,134).unwrap());
        assert!(parse_plan(&combined).is_err());
    }
    #[derive(Clone)]
    struct Commands { replies:Arc<Mutex<VecDeque<String>>>,calls:Arc<Mutex<Vec<Vec<String>>>> }
    impl DeviceCommandPort for Commands {
        async fn execute(&self,_:&Path,args:Vec<String>,_:Duration,_:CancellationToken)->AppResult<ProcessOutput> {
            self.calls.lock().unwrap().push(args);
            Ok(ProcessOutput {success:true,stdout:self.replies.lock().unwrap().pop_front().expect("不得多发动作"),stderr:String::new()})
        }
    }
    fn device(replies:Vec<String>)->(AdbDevice<Commands>,Commands) {
        let commands=Commands {replies:Arc::new(Mutex::new(replies.into())),calls:Arc::default()};
        (AdbDevice::with_commands(AndroidTools {adb:"mock-never-executed".into()},commands.clone()),commands)
    }
    fn response(value:serde_json::Value)->String {format!("Result: Bundle[{{result={}}}]",STANDARD.encode(serde_json::to_vec(&value).unwrap()))}
    #[tokio::test]
    async fn prepare_uses_only_home_provider_and_requires_correlated_receipt_without_replay() {
        let request=uuid::Uuid::now_v7().to_string();
        let good=json!({"status":"prepared","requestId":request,"data":{"protocolVersion":1,"packageName":XIAOXIN_PACKAGE,"removedTaskId":22,"removalRequested":true}});
        let (adb,commands)=device(vec![response(good.clone())]);
        prepare(&adb,"192.0.2.1",22,&request).await.unwrap();
        let calls=commands.calls.lock().unwrap();assert_eq!(calls.len(),1);
        assert!(calls[0].windows(2).any(|pair|pair==["--uri","content://chat.xiaoxin.app.workbench.home"]));
        let payload=calls[0].iter().find_map(|arg|arg.strip_prefix("payload:s:")).unwrap();
        let payload:serde_json::Value=serde_json::from_slice(&STANDARD.decode(payload).unwrap()).unwrap();
        assert_eq!(payload,json!({"protocolVersion":1,"requestId":request,"expectedTaskId":22}));
        drop(calls);
        for bad in [json!({"status":"pending","code":"ACTIVITY_NOT_READY"}),json!({"status":"failed","code":"OUTCOME_UNKNOWN"}),{
            let mut bad=good.clone();bad["requestId"]=json!("different");bad
        },{let mut bad=good;bad["data"]["removedTaskId"]=json!(99);bad}] {
            let (device,commands)=device(vec![response(bad)]);
            assert!(prepare(&device,"192.0.2.1",22,&request).await.is_err());
            assert_eq!(commands.calls.lock().unwrap().len(),1);
        }
    }
    #[tokio::test]
    async fn old_application_reports_upgrade_before_preparation() {
        let (device,commands)=device(vec![response(json!({"status":"ok","data":{"protocolVersion":1,"packageName":XIAOXIN_PACKAGE}}))]);
        assert!(require_capability(&device,"192.0.2.1").await.unwrap_err().to_string().contains("升级"));
        let calls=commands.calls.lock().unwrap();assert_eq!(calls.len(),1);assert!(calls[0].iter().any(|v|v=="getCapabilities"));
    }
    #[tokio::test]
    async fn removal_wait_and_interrupted_recovery_only_read_task_ids() {
        let before=ordinary(ACTIVE);
        let after=ACTIVE.replace("#22","#99").replace("t22","t99");
        let (device,commands)=device(vec![before.clone(),after.clone(),after]);
        assert!(wait_removed(&device,"192.0.2.1",22,Duration::from_secs(1),CancellationToken::new()).await.unwrap());
        let mut evidence=json!({"prepareTaskId":22,"preparation":"unknown","activation":"unknown"});
        verify_preparation(&device,"192.0.2.1",&mut evidence).await.unwrap();
        assert_eq!(evidence["preparation"],"verified");assert_eq!(evidence["taskRemovalReadback"],"succeeded");
        assert_eq!(evidence["activation"],"unknown");
        assert!(commands.calls.lock().unwrap().iter().all(|args|args.ends_with(&["dumpsys".into(),"activity".into(),"activities".into()])));
        assert!(!task_removed(&before,22).unwrap());assert!(task_removed("truncated",22).is_err());
    }
}
