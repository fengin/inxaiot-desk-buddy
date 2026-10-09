# 原生 APK 解析与 Windows 单文件交付

日期：2026-10-09。适用范围：实施人员使用 Windows 工作台维护小新智能屏；Mac 和 Linux 的工具打包同步精简。

## 1. 用户得到的结果

Windows 便携压缩包解压后只有 `INX实施工作台.exe`，不再需要复制旁边的 `tools` 文件夹。程序内包含 ADB、必要 DLL、许可证和版本信息；首次使用设备功能时自动准备到当前用户缓存，后续核对完整后复用。读取 APK 信息不依赖 ADB、Android SDK 或 Java。

Mac 仍交付完整 `.app`，应用包内只保留支持当前架构的 ADB 及必要动态库、许可证，不再携带 APK 外部工具或 Java，也不依赖 Rosetta。Linux 保持可执行文件与精简 ADB 目录的交付形式。

本次没有改动小新应用代码、设备配置、平台数据库结构或业务数据。

## 2. 安装检查规则

- 工作台直接读取 APK 的包名、版本名称、数字版本号、最低 Android 版本、支持架构和启动页面；保持只安装小新的规则，不根据文件名猜测信息。
- 保留实际设备版本、架构、可用空间、批次尺寸及文件是否变化的检查。
- 取消工作台对 APK 的完整验签和升级前证书比对，也不再为此从设备读取或下载已安装 APK。
- 小新 APK 仍按原流程签名发布。Android 安装过程负责判断签名和覆盖安装条件；失败时显示原因，不自动卸载旧应用或清空应用数据。
- 安装后仍读取实际版本和启动状态，再按原规则更新平台版本与操作结果。
- 界面显示“安装包信息已读取”，不再显示“完整性及签名已校验”。

## 3. 程序与缓存

Windows 当前用户缓存位置为 `%LOCALAPPDATA%/com.inxaiot.desk-buddy/tool-cache`。缓存按内嵌工具包内容命名，工作台版本变化但 ADB 内容不变时可复用。

首次准备使用文件锁协调不同进程，先写临时目录并校验，再整体发布。复用前同时核对程序内原始清单和缓存文件，不能通过同时修改文件与缓存清单绕过校验。缓存缺失或损坏时重新准备；不覆盖正在使用的工具文件，损坏旧目录移开保留。无法更新被其他程序占用的缓存时给出错误，不擅自停止其他 ADB 进程。

工具缓存与项目数据分开，删除缓存后下次使用会重新准备。程序不向 EXE 所在目录写工具文件。Windows WebView2 的原有运行条件保持。

## 4. 技术位置

以下文件位于工作台工程 `inxaiot-desk-buddy`，都运行在实施人员电脑上。

| 文件或构建命令 | 作用 |
| --- | --- |
| Rust 文件 `src-tauri/src/infrastructure/smart_screen/apk_manifest.rs` | 读取 ZIP 中的二进制清单和必要资源，提取基本信息，不执行包内代码 |
| Rust 文件 `apk.rs` | 文件读取、前后哈希核对及任务副本；不再调用 `aapt`、`apksigner` 或 Java |
| Rust 文件 `maintenance.rs` | 保留设备兼容性、安装与回读，删除两处提前证书比对 |
| Rust 文件 `tool_bundle.rs` | 内嵌资源释放、缓存复用、原始清单与逐文件校验、损坏恢复 |
| Rust 构建文件 `src-tauri/build.rs` | 将 Windows ADB 压缩资源编入程序；正式 Windows 构建缺少资源时失败 |
| Node.js 脚本 `scripts/package-screen-tools.mjs` | 只准备 ADB、必要动态库、许可证及清单 v2，检查系统、架构和实际可运行性 |
| Node.js 脚本 `scripts/build-windows.mjs`，命令 `pnpm build:windows` | 先准备资源，再编译单 EXE，最后在独立进程中验证内嵌工具 |
| Node.js 脚本 `scripts/build-macos.mjs` | 精简 Mac 工具，保留 ADB、动态库与外层应用签名 |
| 流水线文件 `.github/workflows/portable-release.yml` | Windows 压缩包只收一个 EXE；Mac/Linux 不再打入 APK 外部工具和 Java |
| PowerShell 发布与复验脚本 | 内部清单 v5 使用 `portable-exe`，SBOM 和校验文件保留在内部追溯目录 |

APK 解析使用编译进程序的 Rust 库 `zip`、`apk-info-axml`、`apk-info-xml`，不产生额外运行程序。APK 最大文件限制沿用 2 GB，清单读取上限 4 MiB、必要资源表上限 32 MiB，格式或必要字段不完整时提示重新选择文件。

程序命令行 `--verify-package <报告路径> [APK路径]` 供打包验收使用：只验证 ADB 能运行及可选的本机 APK 解析，不连接设备、不读写项目数据。

## 5. Windows 验证结果

| 验证项 | 结果 |
| --- | --- |
| Rust 库测试 | 152 项通过，包含原有安装失败原因解释和新增缓存、清单解析测试 |
| Windows/Mac 打包编排、工具选择与精简 | 26 项 Node.js 测试通过 |
| 前端类型检查、相关代码检查 | 通过；安装提示不再宣称验签完成 |
| 发布流程结构、PowerShell 语法 | 通过；Windows 编译前准备 ADB，压缩包只收一个 EXE |
| 四份真实小新 APK | Rust 解析与原官方工具对照一致，完整包及三种分架构包均通过 |
| 无签名解析样本 | 保留真实二进制清单构造的未签名样本可读取信息；不向设备安装该样本 |
| 错误文件 | 错误应用包名、普通 XML 替代二进制清单、文件发生变化均被拒绝 |
| 独立成品进程 | 中文目录仅有 EXE；清除 SDK、Java、ADB 环境配置，PATH 仅保留系统目录，成功运行内嵌 ADB 并解析四个 APK |
| 缓存 | 全新缓存自动准备，后续复用；文件与清单同时篡改后恢复；两个独立进程并发准备通过 |
| 实际设备只读连接 | 192.168.3.63、192.168.3.70 通过，读取小新 2.0.10-5022；192.168.3.103 本次未完成连接或只读查询 |

不同分架构 APK 的数字版本号不同，验证按每个文件的实际内容对照，不强制等于完整包。

| APK | 版本 | 数字版本号 | 最低系统级别 | 架构 |
| --- | --- | --- | --- | --- |
| 完整包 | 2.0.10 | 5022 | 24 | arm64-v8a、armeabi-v7a、x86_64 |
| arm64-v8a 分包 | 2.0.10 | 7022 | 24 | arm64-v8a |
| armeabi-v7a 分包 | 2.0.10 | 6022 | 24 | armeabi-v7a |
| x86_64 分包 | 2.0.10 | 9022 | 24 | x86_64 |

## 6. 体积与交付

本机优化后的 EXE 为 **19,156,480 字节**；单文件 ZIP 为 **11,061,240 字节**。对照已发布 v0.2.5 Windows ZIP 的 **48,729,156 字节**，压缩包减少 **77.3%**。构建环境不同可能使正式发布文件大小略有变化。

验证包：`.review-tools/native-apk-20261009/INX-desk-buddy-windows-single-exe-preview.zip`。ZIP 内唯一文件为 `INX实施工作台.exe`，ZIP 的 SHA-256 为 `96b295735e2ddcc0d5d1cd8cf829210f8e987609e1b9979baaa7ba7930cee2b2`。

主要本机证据位于 `.review-tools/native-apk-20261009/`：`summary.json`、`standalone-*.json`、`cache-repair.json`、`parallel-*.json`、`adb-readonly.json`。构建和测试日志为 `.review-tools/native-apk-single-exe-delivery-build.log`、`native-apk-rust-tests.log`、`native-apk-packaging-final.log`、`native-apk-real-files-fixed.log`。

## 7. 验证边界

本轮没有重新执行实机安装、卸载、重启或配置修改，没有重跑全部一体机实机部署。安装的错误处理保留自动化验证；两种尺寸的实机仅做 ADB 只读核对。

Mac 构建流程已同步精简并完成模拟编排验证，本轮本机验收未在 Mac 上重新构建或实机运行。上述体积和校验值对应本机验证包；正式交付版本为 v0.2.6，实际发布文件和构建结果以 GitHub Release 及对应流水线为准。
