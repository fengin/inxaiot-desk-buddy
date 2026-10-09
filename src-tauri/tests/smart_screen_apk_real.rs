#[path = "common/screen_test_support.rs"]
mod support;
use inxaiot_desk_buddy_lib::infrastructure::smart_screen::apk;
use tokio_util::sync::CancellationToken;

#[tokio::test]
#[ignore = "仅用Rust解析明确指定的本机APK，不需要SDK、Java或设备连接"]
async fn reads_selected_release_and_rejects_changed_staged_file()
-> Result<(), Box<dyn std::error::Error>> {
    let info = apk::inspect(&support::live_apk_path()?, CancellationToken::new()).await?;
    assert_eq!(info.package_id, "chat.xiaoxin.app");
    assert!(!info.app_version.is_empty());
    assert!(info.app_version_code > 0);
    assert!(!info.abis.is_empty());
    let temp = tempfile::tempdir()?;
    // 从用户指定包保留真实二进制清单，构造不带签名的解析样本；不向设备安装这些样本。
    use std::io::{Read, Write};
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&info.path)?)?;
    let mut manifest = Vec::new();
    archive
        .by_name("AndroidManifest.xml")?
        .read_to_end(&mut manifest)?;
    let make_apk =
        |name: &str, xml: &[u8]| -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
            let path = temp.path().join(name);
            let mut zip = zip::ZipWriter::new(std::fs::File::create(&path)?);
            zip.start_file(
                "AndroidManifest.xml",
                zip::write::SimpleFileOptions::default(),
            )?;
            zip.write_all(xml)?;
            for abi in &info.abis {
                zip.start_file(
                    format!("lib/{abi}/libfixture.so"),
                    zip::write::SimpleFileOptions::default(),
                )?;
                zip.write_all(b"metadata test fixture")?;
            }
            zip.finish()?;
            Ok(path)
        };
    let unsigned = make_apk("unsigned-metadata-only.apk", &manifest)?;
    let read = apk::inspect(&unsigned, CancellationToken::new()).await?;
    assert_eq!(read.app_version, info.app_version);
    assert_eq!(read.app_version_code, info.app_version_code);
    assert_eq!(read.abis, info.abis);
    assert!(
        apk::inspect(
            &make_apk("plain-xml.apk", b"<manifest/>")?,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
    let encodings = [
        (
            b"chat.xiaoxin.app\0".to_vec(),
            b"fake.xiaoxin.app\0".to_vec(),
        ),
        (
            "chat.xiaoxin.app\0"
                .encode_utf16()
                .flat_map(|c| c.to_le_bytes())
                .collect(),
            "fake.xiaoxin.app\0"
                .encode_utf16()
                .flat_map(|c| c.to_le_bytes())
                .collect(),
        ),
    ];
    let (at, replacement) = encodings
        .iter()
        .find_map(|(old, replacement)| {
            manifest
                .windows(old.len())
                .position(|w| w == old)
                .map(|at| (at, replacement))
        })
        .ok_or("测试包没有可替换的包名")?;
    manifest[at..at + replacement.len()].copy_from_slice(replacement);
    assert!(
        apk::inspect(
            &make_apk("other-app.apk", &manifest)?,
            CancellationToken::new()
        )
        .await
        .is_err()
    );
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
        "{}：版本{}，数字版本{}，最低SDK{}，架构{:?}，Rust解析通过",
        info.name, info.app_version, info.app_version_code, info.min_sdk, info.abis
    );
    Ok(())
}
