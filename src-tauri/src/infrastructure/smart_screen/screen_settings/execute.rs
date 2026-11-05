use super::*;

fn step_mut<'a>(result: &'a mut ScreenTargetResult, code: &str) -> &'a mut Value {
    &mut result.evidence["screenSettings"]["items"][code]
}
fn set_step(result: &mut ScreenTargetResult, code: &str, state: &str, message: &str) {
    let item = step_mut(result, code);
    item["state"] = json!(state);
    item["message"] = json!(message);
}
fn aggregate(result: &mut ScreenTargetResult) {
    let reboot = &result.evidence["screenSettings"]["reboot"];
    let reboot_pending = reboot["requested"] == true
        && !(reboot["bootCompleted"] == true
            && reboot["afterBootId"].as_str().is_some_and(|after| {
                !after.is_empty() && Some(after) != reboot["beforeBootId"].as_str()
            }));
    let states = CODES
        .iter()
        .map(|code| {
            result.evidence["screenSettings"]["items"][code]["state"]
                .as_str()
                .unwrap_or("unknown")
        })
        .collect::<Vec<_>>();
    result.device = if reboot_pending
        || result.evidence["screenSettings"]["verificationPending"] == true
        || states.iter().any(|v| matches!(*v, "unknown" | "pending"))
    {
        ResultState::Unknown
    } else if result.evidence["screenSettings"]["cancelled"] == true {
        ResultState::Cancelled
    } else if states.contains(&"failed") {
        ResultState::Failed
    } else if states.contains(&"not_started") {
        if !result.evidence["screenSettings"]["stoppedAt"].is_null()
            || result.evidence["screenSettings"]["recoveryChecked"] == true
        {
            ResultState::Failed
        } else {
            ResultState::Pending
        }
    } else if states
        .iter()
        .all(|v| matches!(*v, "skipped" | "not_selected"))
    {
        ResultState::Skipped
    } else {
        ResultState::Succeeded
    };
    result.message = match result.device {
        ResultState::Succeeded => "所选智能屏设置均已完成并核实",
        ResultState::Skipped => "所选项目均已满足，无需修改",
        ResultState::Unknown => "已有设置或重启结果待核实；未自动重放或接续后续项目",
        ResultState::Cancelled => "后续未开始项目已停止",
        _ => "某项未完成，已停止该屏后续项目；已完成项保留",
    }
    .into();
}
fn progress_counts(result: &ScreenTargetResult) -> AppResult<(u64, usize, usize)> {
    let patch: ScreenSettingsPatch =
        serde_json::from_value(result.evidence["screenSettings"]["patch"].clone())
            .map_err(|_| error("原设置选项不完整，不能计算执行进度"))?;
    let selected = CODES.iter().filter(|code| patch.selected(code)).count();
    let completed = CODES
        .iter()
        .filter(|code| {
            patch.selected(code)
                && matches!(
                    result.evidence["screenSettings"]["items"][**code]["state"].as_str(),
                    Some("succeeded" | "skipped")
                )
        })
        .count();
    Ok((
        if selected == 0 {
            0
        } else {
            (completed * 100 / selected) as u64
        },
        completed,
        selected,
    ))
}
async fn persist(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    result: &mut ScreenTargetResult,
    phase: &str,
) -> AppResult<()> {
    result.evidence["phase"] = json!(format!("screen_settings_{phase}"));
    aggregate(result);
    task_data::save_target(state.local_store.pool(), &plan.project_id, id, result).await?;
    let screen = plan
        .targets
        .iter()
        .find(|v| v.id == result.screen_id)
        .ok_or_else(|| error("进度目标不属于原计划"))?;
    let (percent, completed, selected) = progress_counts(result)?;
    let stage = if phase.starts_with("adb") {
        "设置ADB"
    } else if phase.starts_with("ntp") {
        "设置及核实NTP"
    } else if phase.starts_with("reboot") {
        "共享重启及连接核实"
    } else if phase.starts_with("time") || phase.starts_with("hardware") {
        "校准系统及硬件时钟"
    } else if phase.starts_with("sleep") || phase.starts_with("wake") {
        "关闭屏睡眠"
    } else if phase.starts_with("recovery") {
        "只读核实原设置结果"
    } else {
        "设置智能屏"
    };
    super::super::installation_progress::ScreenOperationProgress {
        state,
        task: id,
        screen,
    }
    .report(
        stage,
        percent,
        &format!("{stage}；已完成所选项目{completed}/{selected}项"),
        true,
    )
    .await
}
async fn authorize(
    state: &FormalAppState,
    plan: &ScreenPlan,
    held: Option<&HeldScreenLeases>,
    cancel: &CancellationToken,
) -> AppResult<()> {
    check_cancel(cancel)?;
    maintenance_device::authorize(state, plan, held).await?;
    check_cancel(cancel)
}
async fn authorize_identity<P: DeviceCommandPort>(
    state: &FormalAppState,
    plan: &ScreenPlan,
    held: Option<&HeldScreenLeases>,
    cancel: &CancellationToken,
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    original: &ScreenObservation,
) -> AppResult<()> {
    check_cancel(cancel)?;
    let current = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !reliable_identity(screen, &current) || current.observed_mac != original.observed_mac {
        return Err(error("写入前原设备身份已变化，未继续设置"));
    }
    authorize(state, plan, held, cancel).await
}
fn initialize(
    result: &mut ScreenTargetResult,
    patch: &ScreenSettingsPatch,
    row: &ScreenSettingsRead,
    boot: &str,
    observed: &ScreenObservation,
) {
    let reasons = ["adb", "ntp"]
        .into_iter()
        .filter(|code| {
            row.items
                .get(*code)
                .is_some_and(|v| v.state == "ready" && v.reboot_required)
        })
        .collect::<Vec<_>>();
    let items=CODES.iter().map(|code|{
        let item=&row.items[*code];let status=if !patch.selected(code){"not_selected"}else if item.state=="skip"&&!matches!(*code,"time"|"sleepOff"){"skipped"}else{"not_started"};
        ((*code).to_string(),json!({"state":status,"message":item.reason,"before":item.current,"target":item.target,"after":null,"evidence":{}}))
    }).collect::<BTreeMap<_,_>>();
    result.evidence = json!({"phase":"screen_settings_checked","action":"screen_settings","before":observed,
        "screenSettings":{"patch":patch,"items":items,"reboot":{"required":!reasons.is_empty(),"requested":false,"beforeBootId":boot,"afterBootId":null,"reasons":reasons}}});
    result.observation = Some(observed.clone());
}

pub async fn execute(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
) -> AppResult<()> {
    execute_with_device(
        state,
        id,
        plan,
        data,
        screen,
        held,
        result,
        cancel,
        &AdbDevice::new(AndroidTools::discover()?),
    )
    .await
}
#[doc(hidden)]
pub async fn execute_with_device<P: DeviceCommandPort>(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    data: &MaintenancePlan,
    screen: &ScreenAsset,
    held: Option<&HeldScreenLeases>,
    result: &mut ScreenTargetResult,
    cancel: CancellationToken,
    device: &AdbDevice<P>,
) -> AppResult<()> {
    let patch = data
        .settings
        .as_ref()
        .and_then(|v| v.get(&screen.id))
        .ok_or_else(|| error("缺少本屏冻结设置选项"))?;
    patch.validate()?;
    authorize(state, plan, held, &cancel).await?;
    let repo = ScreenRepository::new(state.local_store.pool().clone());
    let current = repo.asset(&plan.project_id, &screen.id).await?;
    if !crate::application::smart_screen::operations::same_target(screen, &current)
        || super::super::value_updates::critical_draft(
            &repo.snapshot(&plan.project_id).await?,
            &current,
        )
    {
        return Err(error("设备关键资料已变化，未执行设置"));
    }
    let observed = device.inspect(screen, "inspect", cancel.clone()).await?;
    let original = data
        .observations
        .get(&screen.id)
        .ok_or_else(|| error("缺少原设备身份检查"))?;
    if !reliable_identity(screen, &observed) || observed.observed_mac != original.observed_mac {
        return Err(error("设备身份与预检不一致，未执行设置"));
    }
    let row = inspect(state, &plan.project_id, device, screen, &observed, patch).await?;
    if row.blocked_count > 0 {
        return Err(error("所选设置的执行条件已变化，请重新检查"));
    }
    let boot = maintenance_device::boot_id(device, &screen.fields.ip).await?;
    initialize(result, patch, &row, &boot, &observed);
    persist(state, id, plan, result, "checked").await?;
    let mut active = "adb";
    let outcome:AppResult<()>=async{
        let ip=&screen.fields.ip;
        if row.items["adb"].state=="ready"{
            let before=&row.items["adb"].current;let mode=before["root"].as_str().ok_or_else(||error("ADB管理权限未确认"))?;
            step_mut(result,"adb")["evidence"]=json!({"saveRequested":true,"saveConfirmed":false,"beforePort":before["persistentPort"]});
            set_step(result,"adb","unknown","正在保存ADB端口；尚未重启核实");persist(state,id,plan,result,"adb_save_requested").await?;
            if before["persistentPort"]!="5555"{
                let current=shell(device,ip,&["getprop","persist.adb.tcp.port"],&cancel).await?;
                if Some(current.as_str())!=before["persistentPort"].as_str(){return Err(error("原ADB端口已被外部修改，未覆盖"))}
                authorize_identity(state,plan,held,&cancel,device,screen,&observed).await?;
                maintenance_device::root_command(device,ip,mode,&["setprop","persist.adb.tcp.port","5555"]).await?;
            }
            if shell(device,ip,&["getprop","persist.adb.tcp.port"],&cancel).await?!="5555"{return Err(error("ADB保存回读未通过，不执行后续设置"))}
            step_mut(result,"adb")["evidence"]["saveConfirmed"]=json!(true);
            set_step(result,"adb","pending","ADB端口已保存，等待共用重启验证");persist(state,id,plan,result,"adb_saved").await?;
        }
        active="ntp";
        if row.items["ntp"].state=="ready"{
            let patch_ntp=patch.ntp.as_ref().ok_or_else(||error("缺少NTP目标"))?;
            let before=ntp::read_device(device,ip).await?;
            let needs_reboot=row.items["ntp"].reboot_required;
            let cap=if needs_reboot{Some(ntp::configuration_capability(device,screen,&observed,false).await?)}else{None};
            step_mut(result,"ntp")["before"]=serde_json::to_value(&before).map_err(|_|error("保存NTP基准失败"))?;
            step_mut(result,"ntp")["evidence"]=json!({"targetServer":patch_ntp.server(),"saveRequested":!patch_ntp.matches(&before),"saveConfirmed":false,"activationConfirmed":false,"syncConfirmed":false});
            set_step(result,"ntp","unknown","正在保存NTP目标，尚未确认运行生效");persist(state,id,plan,result,"ntp_save_requested").await?;
            let after=if !patch_ntp.matches(&before){
                let mode=cap.as_ref().and_then(|v|v["root"].as_str()).ok_or_else(||error("NTP管理权限未确认"))?;
                ntp::save_configuration(device,ip,mode,patch_ntp,&before,cancel.clone(),||authorize_identity(state,plan,held,&cancel,device,screen,&observed)).await?
            }else{before.clone()};
            step_mut(result,"ntp")["after"]=serde_json::to_value(after).map_err(|_|error("记录NTP保存回读失败"))?;
            step_mut(result,"ntp")["evidence"]["saveConfirmed"]=json!(true);
            set_step(result,"ntp","pending","NTP设置已回读，等待生效和授时确认");persist(state,id,plan,result,"ntp_saved").await?;
        }
        active="reboot";
        if result.evidence["screenSettings"]["reboot"]["required"]==true{
            // 所有需要的配置都已保存后，才发唯一一次共享重启。
            maintenance_device::capability(device,screen,&observed,"reboot").await?;
            let mode=maintenance_device::root_mode(device,ip).await?;
            authorize(state,plan,held,&cancel).await?;
            maintenance_device::root_command(device,ip,&mode,&["sync"]).await?;
            let before_boot=maintenance_device::boot_id(device,ip).await?;
            if patch.adb && result.evidence["screenSettings"]["items"]["adb"]["state"]=="skipped" {
                set_step(result,"adb","pending","ADB无需改写，将随本次共享重启只读复验");
            }
            result.evidence["screenSettings"]["reboot"]["beforeBootId"]=json!(before_boot);
            result.evidence["screenSettings"]["reboot"]["requested"]=json!(true);
            persist(state,id,plan,result,"reboot_requested").await?;
            authorize(state,plan,held,&cancel).await?;
            let response=device.adb(ip,&["reboot"],Duration::from_secs(20),CancellationToken::new()).await;
            result.evidence["screenSettings"]["reboot"]["responseConfirmed"]=json!(response.as_ref().is_ok_and(|value|value.success));
            // 已发送的重启不因取消而重发；先完成有限的原动作核实。
            super::super::installation_progress::ScreenOperationProgress{state,task:id,screen}
                .wait("等待共享重启后管理连接",progress_counts(result)?.0,"共享重启只发送一次，正在等待原设备恢复",wait_boot(device,screen,&before_boot)).await?;
            confirm_identity(device,screen,result,&cancel).await?;
            result.evidence["screenSettings"]["reboot"]["afterBootId"]=json!(maintenance_device::boot_id(device,ip).await?);
            result.evidence["screenSettings"]["reboot"]["bootCompleted"]=json!(true);
            persist(state,id,plan,result,"reboot_observed").await?;
        }
        // 已保存的ADB和NTP都要核实；一项回读失败不抹去另一项副作用。
        active="reboot_verification";
        let mut verification_error=None;
        for code in ["adb","ntp"]{
            if result.evidence["screenSettings"]["items"][code]["state"]=="pending"{
                if let Err(cause)=verify_one(state,id,&plan.project_id,device,screen,code,result).await{set_step(result,code,"unknown",&cause.to_string());verification_error.get_or_insert(cause);}
                if code=="ntp" {
                    let deadline=tokio::time::Instant::now()+Duration::from_secs(90);
                    let mut last_notice=tokio::time::Instant::now();
                    while result.evidence["screenSettings"]["items"][code]["state"]!="succeeded"
                        && result.evidence["screenSettings"]["items"][code]["state"]!="unknown"
                        && result.evidence["screenSettings"]["items"][code]["evidence"]["activationConfirmed"]==true
                        && tokio::time::Instant::now()<deadline {
                        set_step(result,code,"pending","等待系统新鲜NTP样本，不重复设置或重启");
                        if last_notice.elapsed()>=Duration::from_secs(10){persist(state,id,plan,result,"ntp_waiting_sample").await?;last_notice=tokio::time::Instant::now();}
                        tokio::time::sleep(Duration::from_secs(2)).await;check_cancel(&cancel)?;
                        if let Err(cause)=verify_one(state,id,&plan.project_id,device,screen,code,result).await{set_step(result,code,"unknown",&cause.to_string());verification_error.get_or_insert(cause);break;}
                    }
                }
                persist(state,id,plan,result,"activation_checked").await?;
            }
        }
        if let Some(cause)=verification_error{return Err(cause)}
        if ["adb","ntp"].iter().any(|code|matches!(result.evidence["screenSettings"]["items"][*code]["state"].as_str(),Some("failed"|"unknown"|"pending"))){return Err(error("ADB或NTP尚未完成，停止本屏后续项目"))}
        active="time";authorize(state,plan,held,&cancel).await?;
        if patch.time{
            let current_observed=device.inspect(screen,"inspect",cancel.clone()).await?;
            if !reliable_identity(screen,&current_observed)||current_observed.observed_mac!=observed.observed_mac{return Err(error("重启后设备身份未重新确认"))}
            let mode=maintenance_device::root_mode(device,ip).await?;
            let current=clock::inspect(device,screen,&current_observed,&mode).await?;
            step_mut(result,"time")["before"]=current.clone();
            if current["satisfied"]==true{set_step(result,"time","skipped","系统与启动RTC偏差均不超过15秒，无需校准");step_mut(result,"time")["after"]=current;}
            else{
                maintenance_device::require_date_setting(device,ip).await?;
                authorize_identity(state,plan,held,&cancel,device,screen,&observed).await?;
                let seconds=time::OffsetDateTime::now_utc().unix_timestamp();
                let mut evidence=json!({"phase":"system_clock_requested","requestedSeconds":seconds,"capability":{"root":mode,"clockSettings":current["settings"],"clock":{"rtc":current["rtc"]}}});
                step_mut(result,"time")["evidence"]=evidence.clone();set_step(result,"time","unknown","正在校准系统及启动RTC");persist(state,id,plan,result,"time_requested").await?;
                authorize(state,plan,held,&cancel).await?;clock::set_system(device,ip,&mode,seconds).await?;
                evidence["phase"]=json!("hardware_clock_requested");step_mut(result,"time")["evidence"]=evidence.clone();persist(state,id,plan,result,"hardware_requested").await?;
                clock::set_hardware_with_checkpoint(device,ip,&mode,&current["rtc"],cancel.clone(),||authorize_identity(state,plan,held,&cancel,device,screen,&observed)).await?;
                evidence["phase"]=json!("hardware_clock_set");step_mut(result,"time")["evidence"]=evidence.clone();persist(state,id,plan,result,"hardware_saved").await?;
                let checked=clock::verify(device,ip,&evidence).await?;step_mut(result,"time")["after"]=checked.readings;
                set_step(result,"time",if checked.succeeded{"succeeded"}else{"failed"},&checked.message);
                persist(state,id,plan,result,"time_checked").await?;
                if !checked.succeeded{return Err(error("双时钟校准未通过，未执行后续睡眠设置"))}
            }
            persist(state,id,plan,result,"time_done").await?;
        }
        active="sleepOff";authorize(state,plan,held,&cancel).await?;
        if patch.sleep_off{
            confirm_identity(device,screen,result,&cancel).await?;
            let before=sleep_policy::read(device,ip,cancel.clone()).await?;before.check_conditions()?;
            step_mut(result,"sleepOff")["before"]=json!(before);
            if before.already_disabled(){set_step(result,"sleepOff","skipped","当前已关闭自动休眠，不写入也不唤醒");step_mut(result,"sleepOff")["after"]=json!(before);}
            else{
                step_mut(result,"sleepOff")["evidence"]=json!({"writeRequested":true,"targetMask":before.target_mask(),"wakeRequested":false});
                set_step(result,"sleepOff","unknown","正在设置接电保持唤醒");persist(state,id,plan,result,"sleep_requested").await?;
                sleep_policy::set_mask_with_checkpoint(device,ip,&before,cancel.clone(),||authorize_identity(state,plan,held,&cancel,device,screen,&observed)).await?;
                let mut after=wait_sleep(device,ip,before.target_mask(),&cancel).await?;
                step_mut(result,"sleepOff")["after"]=json!(after);persist(state,id,plan,result,"sleep_saved").await?;
                if after.needs_wake(){
                    step_mut(result,"sleepOff")["evidence"]["wakeRequested"]=json!(true);persist(state,id,plan,result,"wake_requested").await?;
                    authorize(state,plan,held,&cancel).await?;super::super::kiosk::wake(device,ip,cancel.clone()).await?;
                    after=wait_sleep(device,ip,before.target_mask(),&cancel).await?;
                }
                confirm_sleep_child(result,&after)?;
            }
            persist(state,id,plan,result,"sleep_done").await?;
        }
        active="final_observation";
        if let Err(cause)=record_observation(state,id,&plan.project_id,device,screen,result,&cancel).await{
            result.evidence["screenSettings"]["verificationPending"]=json!(true);return Err(cause);
        }
        Ok(())
    }.await;
    if let Err(cause) = outcome {
        let cancelled = matches!(cause, AppError::Cancelled);
        result.evidence["screenSettings"]["cancelled"] = json!(cancelled);
        if CODES.contains(&active) {
            let status = result.evidence["screenSettings"]["items"][active]["state"]
                .as_str()
                .unwrap_or("not_started");
            if !matches!(status, "succeeded" | "skipped" | "not_selected") {
                set_step(
                    result,
                    active,
                    if matches!(status, "unknown" | "pending") {
                        "unknown"
                    } else if cancelled {
                        "not_started"
                    } else {
                        "failed"
                    },
                    &cause.to_string(),
                );
            }
        }
        result.evidence["screenSettings"]["stoppedAt"] = json!(active);
        result.evidence["screenSettings"]["stopReason"] = json!(cause.to_string());
        persist(state, id, plan, result, "stopped").await?;
        result.message = format!("{}；已停止该屏后续项目", cause);
        return Ok(());
    }
    persist(state, id, plan, result, "completed").await
}

async fn confirm_identity<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    result: &ScreenTargetResult,
    cancel: &CancellationToken,
) -> AppResult<ScreenObservation> {
    let observed = device.inspect(screen, "inspect", cancel.clone()).await?;
    if !reliable_identity(screen, &observed)
        || observed.observed_mac.as_deref() != result.evidence["before"]["observedMac"].as_str()
    {
        return Err(error("原设备身份未重新确认，未继续写入或标记成功"));
    }
    Ok(observed)
}
async fn record_observation<P: DeviceCommandPort>(
    state: &FormalAppState,
    id: &str,
    project: &str,
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
    cancel: &CancellationToken,
) -> AppResult<ScreenObservation> {
    let mut observed = confirm_identity(device, screen, result, cancel).await?;
    observed.task_id = Some(id.into());
    observed.operation_type = "screen_settings".into();
    ScreenRepository::new(state.local_store.pool().clone())
        .append_observation(project, &observed)
        .await?;
    result.observation = Some(observed.clone());
    Ok(observed)
}
fn confirm_sleep_child(
    result: &mut ScreenTargetResult,
    after: &sleep_policy::SleepPolicyStatus,
) -> AppResult<()> {
    let evidence = result.evidence["screenSettings"]["items"]["sleepOff"]["evidence"].clone();
    let mut legacy = ScreenTargetResult {
        device: ResultState::Unknown,
        evidence: json!({"action":"sleep_off","sleepOff":evidence}),
        ..Default::default()
    };
    maintenance_device::confirm_sleep_policy(&mut legacy, after)?;
    let state = match legacy.device {
        ResultState::Succeeded => "succeeded",
        ResultState::Failed => "failed",
        _ => "unknown",
    };
    step_mut(result, "sleepOff")["after"] = json!(after);
    set_step(result, "sleepOff", state, &legacy.message);
    Ok(())
}
async fn wait_boot<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    before: &str,
) -> AppResult<()> {
    maintenance_device::wait_boot_with_limit(
        device,
        screen,
        &json!({"bootId":before}),
        Duration::from_secs(180),
        Duration::from_secs(3),
    )
    .await
}
async fn wait_sleep<P: DeviceCommandPort>(
    device: &AdbDevice<P>,
    ip: &str,
    target: u32,
    cancel: &CancellationToken,
) -> AppResult<sleep_policy::SleepPolicyStatus> {
    for index in 0..5 {
        let status = sleep_policy::read(device, ip, cancel.clone()).await?;
        if status.saved == Some(target)
            && status.effective_mask == target
            && status.already_disabled()
        {
            return Ok(status);
        }
        if index < 4 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            check_cancel(cancel)?;
        }
    }
    Err(error("睡眠设置保存或生效未确认，不重复写入"))
}

async fn verify_one<P: DeviceCommandPort>(
    state: &FormalAppState,
    id: &str,
    project: &str,
    device: &AdbDevice<P>,
    screen: &ScreenAsset,
    code: &str,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let ip = &screen.fields.ip;
    let cancel = CancellationToken::new();
    let observed = record_observation(state, id, project, device, screen, result, &cancel).await?;
    match code {
        "adb" => {
            let current = adb_state(device, screen, &observed).await?;
            step_mut(result, code)["after"] = current.clone();
            if current["satisfied"] != true {
                set_step(
                    result,
                    code,
                    "failed",
                    "当前ADB持久或运行端口不符合目标，未重复设置",
                );
            } else if result.evidence["screenSettings"]["reboot"]["requested"] != true {
                set_step(
                    result,
                    code,
                    "failed",
                    "ADB端口已保存，但原任务未执行共享重启验证；未补发重启",
                );
            } else if current["bootId"]
                == result.evidence["screenSettings"]["reboot"]["beforeBootId"]
            {
                set_step(
                    result,
                    code,
                    "unknown",
                    "ADB端口已保存，尚未确认原共享重启完成",
                );
            } else if current["servicePort"] != "5555" {
                set_step(result, code, "failed", "系统已重启但运行端口未达到5555");
            } else if result.evidence["before"]["appRunning"] == true
                && observed.app_running != Some(true)
            {
                set_step(
                    result,
                    code,
                    "failed",
                    "系统和端口已恢复，但原来运行的小新未恢复；未主动重启应用",
                );
            } else {
                set_step(
                    result,
                    code,
                    "succeeded",
                    "已确认共用重启后持久及运行端口均为5555",
                );
            }
        }
        "ntp" => {
            let patch: ScreenSettingsPatch =
                serde_json::from_value(result.evidence["screenSettings"]["patch"].clone())
                    .map_err(|_| error("原设置选项不完整"))?;
            let target = patch.ntp.as_ref().ok_or_else(|| error("原NTP目标缺失"))?;
            let before: NtpConfig = serde_json::from_value(
                result.evidence["screenSettings"]["items"][code]["before"].clone(),
            )
            .map_err(|_| error("原NTP基准缺失"))?;
            let current = ntp::read_device(device, ip).await?;
            step_mut(result, code)["after"] = json!(current);
            if !target.matches(&current)
                || current.time_zone != before.time_zone
                || current.auto_time_zone != before.auto_time_zone
            {
                set_step(
                    result,
                    code,
                    "failed",
                    "当前NTP设置不符合原目标或时区变化；未重放设置",
                );
                return Ok(());
            }
            let boot = maintenance_device::boot_id(device, ip).await?;
            let status = ntp::read_status(device, ip).await?;
            let direct = ntp::source_event(device, ip, target.server(), &status).await;
            let old_boot = result.evidence["screenSettings"]["reboot"]["beforeBootId"]
                .as_str()
                .ok_or_else(|| error("缺少原启动编号"))?;
            let rebooted = result.evidence["screenSettings"]["reboot"]["requested"] == true
                && boot != old_boot
                && shell(device, ip, &["getprop", "sys.boot_completed"], &cancel).await? == "1";
            let active = direct.is_some()
                || (rebooted
                    && result.evidence["screenSettings"]["items"][code]["evidence"]["saveConfirmed"]
                        == true)
                || same_boot_ntp(
                    state,
                    project,
                    screen,
                    target.server(),
                    &boot,
                    status["systemServerPid"].as_u64(),
                    observed.observed_mac.as_deref(),
                )
                .await?;
            let synchronized = active
                && ntp::fresh_sample(&status)
                && observed
                    .clock_offset_seconds
                    .is_some_and(|v| v.unsigned_abs() <= 15);
            step_mut(result, code)["evidence"] = json!({"targetServer":target.server(),"saveConfirmed":true,"afterBootId":boot,"systemServerPid":status["systemServerPid"],"observedMac":observed.observed_mac.as_deref().map(crate::domain::smart_screen::rules::normalize_mac),"activationConfirmed":active,"syncConfirmed":synchronized,"status":status,"sourceEvent":direct});
            set_step(
                result,
                code,
                if synchronized { "succeeded" } else { "failed" },
                if synchronized {
                    "NTP地址、生效来源和新鲜样本均已确认"
                } else {
                    "NTP已保存，但运行生效或自动授时未确认；未重复设置或重启"
                },
            );
        }
        "time" => {
            let evidence = result.evidence["screenSettings"]["items"][code]["evidence"].clone();
            let checked = clock::verify(device, ip, &evidence).await?;
            step_mut(result, code)["after"] = checked.readings;
            set_step(
                result,
                code,
                if checked.succeeded {
                    "succeeded"
                } else {
                    "failed"
                },
                &checked.message,
            );
        }
        "sleepOff" => {
            let after = sleep_policy::read(device, ip, cancel).await?;
            confirm_sleep_child(result, &after)?;
        }
        _ => return Err(error("未知设置项目")),
    }
    Ok(())
}

pub async fn verify(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
) -> AppResult<()> {
    let device = AdbDevice::new(AndroidTools::discover()?);
    verify_with_device(state, id, plan, screen, result, &device).await
}
#[doc(hidden)]
pub async fn verify_with_device<P: DeviceCommandPort>(
    state: &FormalAppState,
    id: &str,
    plan: &ScreenPlan,
    screen: &ScreenAsset,
    result: &mut ScreenTargetResult,
    device: &AdbDevice<P>,
) -> AppResult<()> {
    let observed = device
        .inspect(screen, "inspect", CancellationToken::new())
        .await?;
    if !reliable_identity(screen, &observed)
        || observed.observed_mac.as_deref() != result.evidence["before"]["observedMac"].as_str()
    {
        return Err(error("尚未重新确认原屏身份，未核实结果"));
    }
    if result.evidence["screenSettings"]["reboot"]["requested"] == true {
        let boot = maintenance_device::boot_id(device, &screen.fields.ip).await?;
        if Some(boot.as_str())
            != result.evidence["screenSettings"]["reboot"]["beforeBootId"].as_str()
            && shell(
                device,
                &screen.fields.ip,
                &["getprop", "sys.boot_completed"],
                &CancellationToken::new(),
            )
            .await?
                == "1"
        {
            result.evidence["screenSettings"]["reboot"]["afterBootId"] = json!(boot);
            result.evidence["screenSettings"]["reboot"]["bootCompleted"] = json!(true);
        }
    }
    for code in CODES {
        if matches!(
            result.evidence["screenSettings"]["items"][code]["state"].as_str(),
            Some("unknown" | "pending")
        ) {
            if let Err(cause) =
                verify_one(state, id, &plan.project_id, device, screen, code, result).await
            {
                set_step(result, code, "unknown", &cause.to_string());
            }
            persist(state, id, plan, result, "recovery_read").await?;
        }
    }
    let _ = observed;
    record_observation(
        state,
        id,
        &plan.project_id,
        device,
        screen,
        result,
        &CancellationToken::new(),
    )
    .await?;
    result.evidence["screenSettings"]["verificationPending"] = json!(false);
    result.evidence["screenSettings"]["recoveryChecked"] = json!(true);
    persist(state, id, plan, result, "recovery_checked").await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn result(states: [&str; 4]) -> ScreenTargetResult {
        let items = CODES
            .iter()
            .zip(states)
            .map(|(code, state)| {
                (
                    (*code).to_string(),
                    json!({"state":state,"message":"原结果"}),
                )
            })
            .collect::<BTreeMap<_, _>>();
        ScreenTargetResult {
            evidence: json!({"screenSettings":{"items":items}}),
            ..Default::default()
        }
    }
    #[test]
    fn unknown_effects_dominate_later_success_and_cancellation() {
        let mut value = result(["pending", "succeeded", "not_started", "not_started"]);
        value.evidence["screenSettings"]["cancelled"] = json!(true);
        aggregate(&mut value);
        assert_eq!(value.device, ResultState::Unknown);
        assert_eq!(
            value.evidence["screenSettings"]["items"]["ntp"]["state"],
            "succeeded"
        );
    }

    #[test]
    fn only_ntp_starts_at_zero_and_unselected_items_never_raise_progress() {
        let mut value = result([
            "not_selected",
            "not_started",
            "not_selected",
            "not_selected",
        ]);
        value.evidence["screenSettings"]["patch"] =
            json!({"adb":false,"time":false,"sleepOff":false,"ntp":{"server":"time.test"}});
        assert_eq!(progress_counts(&value).unwrap(), (0, 0, 1));
        set_step(&mut value, "adb", "succeeded", "不在本次选项中");
        assert_eq!(progress_counts(&value).unwrap(), (0, 0, 1));
        set_step(&mut value, "ntp", "succeeded", "已确认");
        assert_eq!(progress_counts(&value).unwrap(), (100, 1, 1));
    }

    #[test]
    fn four_selected_items_count_only_confirmed_success_or_skip() {
        let mut value = result(["not_started"; 4]);
        value.evidence["screenSettings"]["patch"] =
            json!({"adb":true,"time":true,"sleepOff":true,"ntp":{"server":"time.test"}});
        assert_eq!(progress_counts(&value).unwrap(), (0, 0, 4));
        set_step(&mut value, "adb", "succeeded", "已确认");
        set_step(&mut value, "ntp", "skipped", "已满足");
        assert_eq!(progress_counts(&value).unwrap(), (50, 2, 4));
        set_step(&mut value, "time", "succeeded", "已确认");
        set_step(&mut value, "sleepOff", "skipped", "已满足");
        assert_eq!(progress_counts(&value).unwrap(), (100, 4, 4));
    }
    #[test]
    fn recovery_never_restarts_unexecuted_steps_and_can_close_protection() {
        let mut value = result(["failed", "succeeded", "not_started", "not_started"]);
        value.evidence["screenSettings"]["recoveryChecked"] = json!(true);
        aggregate(&mut value);
        assert_eq!(value.device, ResultState::Failed);
        assert_eq!(
            value.evidence["screenSettings"]["items"]["time"]["state"],
            "not_started"
        );
        assert_eq!(
            value.evidence["screenSettings"]["items"]["ntp"]["state"],
            "succeeded"
        );
    }
    #[test]
    fn pending_final_identity_keeps_successful_items_but_requires_read_only_review() {
        let mut value = result(["succeeded", "succeeded", "skipped", "succeeded"]);
        value.evidence["screenSettings"]["verificationPending"] = json!(true);
        aggregate(&mut value);
        assert_eq!(value.device, ResultState::Unknown);
        value.evidence["screenSettings"]["verificationPending"] = json!(false);
        aggregate(&mut value);
        assert_eq!(value.device, ResultState::Succeeded);
    }
    #[test]
    fn shared_reboot_need_is_frozen_before_any_dispatch() {
        let patch = ScreenSettingsPatch {
            ntp: Some(NtpPatch {
                server: "time.test".into(),
            }),
            ..Default::default()
        };
        let row = ScreenSettingsRead {
            screen_id: "screen".into(),
            read_at: now(),
            pending_count: 2,
            blocked_count: 0,
            error: None,
            items: CODES
                .iter()
                .map(|code| {
                    (
                        (*code).into(),
                        item(
                            if matches!(*code, "adb" | "ntp") {
                                "ready"
                            } else {
                                "skip"
                            },
                            json!({}),
                            json!({}),
                            "原值".into(),
                            "目标",
                            "原因",
                            matches!(*code, "adb" | "ntp"),
                        ),
                    )
                })
                .collect(),
        };
        let mut value = result(["not_started"; 4]);
        initialize(
            &mut value,
            &patch,
            &row,
            "boot-before",
            &ScreenObservation::default(),
        );
        assert_eq!(value.evidence["screenSettings"]["reboot"]["required"], true);
        assert_eq!(
            value.evidence["screenSettings"]["reboot"]["requested"],
            false
        );
        assert_eq!(
            value.evidence["screenSettings"]["reboot"]["reasons"],
            json!(["adb", "ntp"])
        );
        assert_eq!(
            value.evidence["screenSettings"]["items"]["time"]["state"],
            "not_started"
        );
    }
    #[test]
    fn sleep_child_uses_original_saved_and_active_boundaries_without_waking() {
        let mut value = result(["not_selected", "not_selected", "not_selected", "unknown"]);
        step_mut(&mut value, "sleepOff")["evidence"] =
            json!({"writeRequested":true,"targetMask":7,"wakeRequested":true});
        let mut status = sleep_policy::SleepPolicyStatus {
            saved: Some(7),
            saved_raw: "7".into(),
            effective_mask: 7,
            is_powered: true,
            plug_type: 1,
            stay_on: true,
            device_admin_enforced: false,
            device_admin_timeout_ms: i64::MAX as u64,
            wakefulness: "Asleep".into(),
            display: "OFF".into(),
            screen_off_timeout_ms: 60000,
        };
        confirm_sleep_child(&mut value, &status).unwrap();
        assert_eq!(
            value.evidence["screenSettings"]["items"]["sleepOff"]["state"],
            "succeeded"
        );
        status.is_powered = false;
        status.plug_type = 0;
        status.stay_on = false;
        confirm_sleep_child(&mut value, &status).unwrap();
        assert_eq!(
            value.evidence["screenSettings"]["items"]["sleepOff"]["state"],
            "unknown"
        );
    }
}
