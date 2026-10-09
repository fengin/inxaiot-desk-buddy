fn main() {
    embed_adb();
    let commit = std::process::Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unknown".into());
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .ok()
        .is_some_and(|output| output.status.success() && !output.stdout.is_empty());
    println!(
        "cargo:rustc-env=INX_BUILD_GIT_COMMIT={}{}",
        commit,
        if dirty { "-dirty" } else { "" }
    );
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/index");
    tauri_build::build()
}

fn embed_adb() {
    use std::{env, fs, path::PathBuf};
    println!("cargo:rerun-if-env-changed=INX_EMBEDDED_ADB");
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let windows = env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows");
    let source = env::var_os("INX_EMBEDDED_ADB").filter(|_| windows);
    let bytes = if let Some(path) = source {
        let path = PathBuf::from(path);
        println!("cargo:rerun-if-changed={}", path.display());
        let bytes = fs::read(path).expect("读取内嵌 ADB 包失败，请运行 pnpm build:windows");
        assert!(
            bytes.starts_with(&[0x1f, 0x8b]) && bytes.len() < 32 * 1024 * 1024,
            "内嵌 ADB 包无效"
        );
        bytes
    } else {
        assert!(
            !windows || env::var("PROFILE").as_deref() != Ok("release"),
            "Windows 正式构建必须内嵌 ADB，请使用 pnpm build:windows"
        );
        Vec::new()
    };
    fs::write(output.join("embedded-adb.json.gz"), bytes).expect("写入内嵌 ADB 构建资源");
}
