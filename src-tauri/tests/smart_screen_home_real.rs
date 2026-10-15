use inxaiot_desk_buddy_lib::infrastructure::smart_screen::{
    device::{AdbDevice, AndroidTools},
    home::{self, HomeWrite},
};
use tokio_util::sync::CancellationToken;

/// 仅在明确指定的一台授权测试屏执行；没有默认目标，不自动配置设备所有者。
#[tokio::test]
#[ignore = "需显式设置 INX_HOME_TEST_TARGET 和可用的测试 ADB 服务；会持久设置该测试屏主屏"]
async fn persistent_home_protocol_real() -> Result<(), Box<dyn std::error::Error>> {
    let ip = std::env::var("INX_HOME_TEST_TARGET")
        .map_err(|_| "请显式设置 INX_HOME_TEST_TARGET 为一台授权测试屏 IP")?;
    if ![
        "192.168.3.82",
        "192.168.3.74",
        "192.168.3.70",
        "192.168.3.106",
    ]
    .contains(&ip.as_str())
    {
        return Err("本测试只允许本次授权的测试屏地址".into());
    }
    let device = AdbDevice::new(AndroidTools::discover()?);
    device.connect(&ip, CancellationToken::new()).await?;
    let before = home::query(&device, &ip).await?;
    if before.verified() {
        println!("小新已是持久主屏，跳过策略写入");
    } else {
        match home::set_home(&device, &ip).await? {
            HomeWrite::Applied => println!("持久主屏策略 API 已返回成功，开始独立读取系统结果"),
            HomeWrite::Rejected(message) => return Err(message.into()),
        }
    }
    let after = home::wait_verified(&device, &ip, std::time::Duration::from_secs(20)).await?;
    assert!(after.verified(), "系统未确认小新持久主屏：{after:?}");
    // 重复核实必须仍是纯读取；不再次提交策略，也不重启应用或系统。
    assert!(home::query(&device, &ip).await?.verified());
    println!("系统持久规则及当前主屏解析均已确认");
    if std::env::var("INX_HOME_TEST_CLEAR").as_deref() == Ok("1") {
        match home::clear_home(&device, &ip).await? {
            HomeWrite::Applied => println!("撤销 API 已返回成功，开始独立读取系统结果"),
            HomeWrite::Rejected(message) => return Err(message.into()),
        }
        let cleared =
            home::wait_for_target(&device, &ip, false, std::time::Duration::from_secs(20)).await?;
        assert!(
            cleared.matches_target(false),
            "未确认持久主屏已撤销：{cleared:?}"
        );
        // 只有清除结果已确认后才执行明确的恢复操作；未知结果直接终止，不自动补写。
        match home::set_home(&device, &ip).await? {
            HomeWrite::Applied => println!("清除验证完成，恢复测试屏的小新持久主屏"),
            HomeWrite::Rejected(message) => return Err(message.into()),
        }
        let restored =
            home::wait_for_target(&device, &ip, true, std::time::Duration::from_secs(20)).await?;
        assert!(
            restored.matches_target(true),
            "未确认测试屏持久主屏已恢复：{restored:?}"
        );
    }
    Ok(())
}
