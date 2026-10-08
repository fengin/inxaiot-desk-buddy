#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::infrastructure::smart_screen::apk;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "使用本机 Android SDK、Java，仅解析明确指定的当前安装包和检查文件完整性"]
async fn reads_selected_release_and_rejects_changed_staged_file()
-> Result<(), Box<dyn std::error::Error>> {
    let info = apk::inspect(&support::live_apk_path()?, CancellationToken::new()).await?;
    assert_eq!(info.package_id, "chat.xiaoxin.app");
    assert!(!info.app_version.is_empty());
    assert!(info.app_version_code > 0);
    assert!(!info.abis.is_empty());
    assert!(!info.signer_sha256.is_empty());
    let temp = tempfile::tempdir()?;
    let staged = apk::stage(&info, temp.path()).await?;
    assert_eq!(
        apk::sha256(std::path::Path::new(&staged.path)).await?,
        info.sha256
    );
    let mut changed = staged.clone();
    changed.sha256 = "0".repeat(64);
    assert!(
        apk::stage(&changed, &temp.path().join("reject"))
            .await
            .is_err()
    );
    eprintln!(
        "{}：版本{}，数字版本{}，最低SDK{}，架构{:?}，签名有效",
        info.name, info.app_version, info.app_version_code, info.min_sdk, info.abis
    );
    Ok(())
}
