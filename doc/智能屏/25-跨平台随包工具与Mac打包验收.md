# 跨平台随包工具与 Mac 打包验收

日期：2026-10-08。目的：实施人员拿到适合自己电脑的工作台包后，使用随包工具管理智能屏，不安装开发 SDK，也不把 Windows 工具复制到 Mac 使用。

## 打包规则

| 工作台运行环境 | Android 工具 | Java | 交付位置 |
| --- | --- | --- | --- |
| Windows x64 | Windows `adb.exe`、配套 DLL、Windows APK 工具 | x64 Java 21 | 主程序旁的 `tools` |
| macOS Intel | macOS `adb`、macOS APK 工具 | x64 Java 21 | `.app/Contents/Resources/tools` |
| macOS Apple Silicon | macOS `adb`、macOS APK 工具 | arm64 Java 21 | `.app/Contents/Resources/tools` |
| Linux x64 | Linux `adb`、Linux APK 工具 | x64 Java 21 | 主程序旁的 `tools` |

ADB、APK 解析和验签工具、Java 及许可证必须完整随包。工具来源和安装方式见 [Android Platform-Tools 官方说明](https://developer.android.com/tools/releases/platform-tools)及 [sdkmanager 官方说明](https://developer.android.com/tools/sdkmanager)。基线保持 platform-tools 35 及以上、build-tools 36.1.0、Temurin 或 JetBrains Java 21。

工具包清单记录适配的工作台系统与架构，并分别记录实际工具架构。Windows x64 允许已验证的 32 位 Android 工具，Java 使用 x64。官方 macOS ADB 从 32.0.0 起提供 Universal 通用文件，包含 Intel x86_64 和 Apple Silicon arm64；本项目使用 35 及以上版本，不另外维护两套 ADB，打包要求其包含目标架构。Apple Silicon 的 APK 解析工具 `aapt` 如果只有 Intel 版本，构建电脑和使用电脑都需要 Rosetta，清单及包内 `README-macos.txt` 会注明。Java 始终匹配工作台目标架构。

## Mac 构建

在对应架构的 Mac 准备 Android SDK 和 Java 路径后执行 `pnpm build:macos`。命令自动准备并检查 Mac 工具、编译应用、确认应用架构、附加工具、完成签名和清单复验。Intel 与 Apple Silicon 分别生成应用，不将一个架构的应用标作通用包。Windows 上调用此命令会明确拒绝，不生成伪装成 Mac 的 Windows 产物。

已有多平台流水线使用同一 Mac 构建命令，先准备目标系统的 SDK 和 Java。Windows、Mac、Linux 工具准备分别明确系统；工具清单和运行时都检查平台、架构，文件哈希检查继续保留。

## 代码位置

以下文件都在工作台工程 `inxaiot-desk-buddy`：

| 文件 | 职责 |
| --- | --- |
| Node.js 脚本 `scripts/package-screen-tools.mjs` | 接收 `--platform`、`--arch`，检查并复制工具，实际运行及生成清单 |
| Node.js 文件 `scripts/screen-tool-platform.mjs` | 识别 Windows PE、macOS Mach-O、Linux ELF 文件格式及处理器架构，防止只改文件名混包 |
| Node.js 脚本 `scripts/build-macos.mjs` | Mac 完整应用构建、工具装入、签名和最终校验 |
| 流水线文件 `.github/workflows/portable-release.yml` | 分系统准备 SDK，分 Intel/Apple Silicon 构建 Mac 包 |
| PowerShell 脚本 `scripts/package-screen-tools.ps1`、`release-internal.ps1`、`verify-internal-release.ps1` | Windows 打包和复验明确使用 win32/x64 目标 |
| Rust 文件 `src-tauri/src/infrastructure/smart_screen/tool_bundle.rs` | 工具目录定位、当前工作台平台/架构匹配及逐文件完整性检查 |

## 已验证与待验收

| 验证 | 结果 |
| --- | --- |
| 打包平台、文件格式、架构和错误清单 | 8 项 Node.js 测试通过 |
| Mac 构建顺序、逐个工具/动态库签名、清单顺序、失败停止及架构拒绝 | 16 项模拟编排测试通过 |
| Rust 运行时平台、架构、缺失/损坏文件及目录规则 | 4 项测试通过 |
| Windows 现有工具版本检查、复制后的清单重算及复验 | 通过；ADB 35.0.2、build-tools 36.1.0、JetBrains Java 21.0.8 |
| 清除开发环境后的独立 Windows 进程 | 1 项通过；只用随包工具解析小新 2.0.9-5019 并验证签名 |
| JavaScript 代码检查、PowerShell 语法和差异检查 | 通过 |

Windows 独立验证记录位于 `.review-tools/platform-tools-20261008/`。未连接屏、重启设备或写入项目数据库；没有修改原 v8 工具目录。

本轮没有 Mac 主机，也没有生成或验证实际 Mac `.app`。后续分别在 Intel、Apple Silicon Mac 完成：实际构建；核对 ADB、APK 工具、Java 及其动态库签名和执行权限；中文路径、无开发 SDK 环境下运行；按清单验证 Rosetta 条件；实际连接屏完成只读检查和 APK 解析。需要正式分发时，另完成 Developer ID 签名及公证验收，不能用 Windows 模拟编排代替。
