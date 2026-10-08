use inxaiot_desk_buddy_lib::infrastructure::smart_screen::{
    apk::{self, PackageTools},
    device::AndroidTools,
    tool_bundle::{bundle_root, executable_name},
};

#[tokio::test]
#[ignore = "复制该测试程序到含tools的验收目录，以清除SDK和Java环境变量的独立进程执行，只读解析用户APK"]
async fn bundled_android_tools_work_without_developer_environment()
-> Result<(), Box<dyn std::error::Error>> {
    for variable in [
        "JAVA_HOME",
        "ANDROID_HOME",
        "ANDROID_SDK_ROOT",
        "INX_ADB_PATH",
        "INX_AAPT_PATH",
        "INX_APKSIGNER_JAR",
    ] {
        assert!(
            std::env::var_os(variable).is_none(),
            "验收进程仍设置{variable}"
        );
    }
    let root = bundle_root(&std::env::current_exe()?)?;
    let android = AndroidTools::discover()?;
    let package = PackageTools::discover(&android)?;
    assert_eq!(android.adb, root.join("android").join(executable_name("adb")));
    assert_eq!(package.java, root.join("java/bin").join(executable_name("java")));
    assert_eq!(package.aapt, root.join("android-build").join(executable_name("aapt")));
    let path = std::env::var_os("INX_STEP7_APK").ok_or("缺少验收APK")?;
    let apk = apk::inspect(
        std::path::Path::new(&path),
        tokio_util::sync::CancellationToken::new(),
    )
    .await?;
    assert_eq!(apk.package_id, "chat.xiaoxin.app");
    assert!(!apk.app_version.is_empty());
    assert!(apk.app_version_code > 0);
    assert!(!apk.signer_sha256.is_empty());
    println!(
        "SCREEN_BUNDLED_TOOLS_PASS 小新{}-{}，签名验证通过",
        apk.app_version, apk.app_version_code
    );
    Ok(())
}

#[test]
#[ignore = "验收脚本临时移走或损坏工具后运行，要求直接报告随包工具错误，不能回退到开发环境"]
fn missing_or_corrupted_bundle_is_reported() {
    let error = AndroidTools::discover()
        .expect_err("随包工具异常未被发现")
        .to_string();
    assert!(
        error.contains("随附 Android 工具"),
        "错误提示未说明随附工具：{error}"
    );
    println!("SCREEN_BUNDLED_TOOLS_ERROR_PASS");
}
