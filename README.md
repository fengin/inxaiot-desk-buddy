# INX 实施工作台（重构版）

面向项目实施与维护人员的桌面工作台，根据 `inxaiot-edge-workbench` 的实际能力重新进行产品、技术和工程设计。

当前应用版本：**0.2.5**。功能与验收范围见[版本变更记录](CHANGELOG.md)。

Windows 与现有两台一体机、三台智能屏的整体实机回归已完成，包括一体机部署升级、智能屏安装及配置管理、结果回写和异常恢复。验证结果与范围见[整体实机回归记录](doc/27-整体实机回归记录.md)；Mac 原生运行及现场触摸、语音体验仍需在目标设备验收。

当前部署物模型以项目共享配置为中心：`.env`、`docker-compose.yml`、`host-info.json`三个模板及发布参数保存到项目`inxaiot_desk_buddy`，Compose决定可部署服务。首次部署和整包升级不再选择Release目录或要求用户准备`manifest.json`，只为Compose服务选择单镜像tar并确认RepoTag；工作台在上传前完成模板、YAML、端口、镜像和逐节点渲染校验，再自动生成Agent使用的内部发布包。

## 文档

- [文档总索引](doc/README.md)
- [一体机管理文档](doc/一体机/README.md)
- [智能屏运维管理需求设计](doc/智能屏/01-智能屏运维管理需求设计.md)
- [智能屏技术方案设计](doc/智能屏/05-智能屏技术方案设计.md)
- [智能屏开发与验收计划](doc/智能屏/06-智能屏开发与验收计划.md)
- [跨业务扩展技术债与实施顺序](doc/20-跨业务扩展技术债与实施顺序.md)
- [一体机开发阶段总结与代码 Review 交接（2026-09-06）](doc/一体机/18-开发阶段总结与代码Review交接.md)
- [重构背景](doc/重构背景.md)
- [桌面工作台界面设计规范](doc/01-桌面工作台界面设计规范.md)
- [产品设计文档](doc/02-产品设计文档.md)
- [技术方案设计](doc/03-技术方案设计.md)
- [界面Demo验收说明](doc/04-界面Demo验收说明.md)
- [开发实施与验收记录](doc/05-开发实施与验收记录.md)
- [阶段7多实例、恢复、性能与真实环境验收记录](doc/13-阶段7多实例恢复性能与真实环境验收记录.md)
- [架构与产品闭环Review问题及整改计划](doc/14-架构与产品闭环Review问题及整改计划.md)

## 智能屏正式功能

普通Tauri桌面已接入智能屏真实功能：平台及本机资产、空间树、草稿、检查、注册更新、人工合并、小新安装、屏端配置管理、版本与状态同步、校时、端口保持、重启及诊断。页面使用正式调用接口，不返回模拟执行结果。

开发与验收范围见[智能屏开发计划](doc/智能屏/06-智能屏开发与验收计划.md)。运维过程和诊断文件保存在本机，注册屏操作结果存工作台专用库，确认后的业务结果存项目平台库。屏端应用配置通过受限 ADB 通道读取、修改、按需重启和回读，联调基线为小新 2.0.10；具体范围见[屏端配置管理实施与验收](doc/智能屏/19-屏端配置管理实施与验收.md)。

步骤7已完成整体流程、双进程操作锁、异常恢复和一体机相关回归。2026-10-08 已补齐跨版本升级、两台 4 寸同批安装及三屏配置修改和还原验证，详见[整体实机回归记录](doc/27-整体实机回归记录.md)。屏首次安装由厂家准备，不属于本轮必验范围；后续验收边界见[接续计划](doc/智能屏/20-智能屏开发测试遗留与接续计划.md)。

智能屏发布必须携带适合运行电脑的工具。已有打包脚本 `scripts/package-screen-tools.mjs` 按 `--platform` 和 `--arch` 参数复制 ADB（Android 调试桥）、APK 解析和验签工具、Java 21 及许可证，校验操作系统、处理器架构和文件完整性，并实际运行工具检查。工具基线为 platform-tools 35 及以上、build-tools 36.1.0 和 Java 21。

| 运行电脑 | 随包工具 | 工具位置 |
| --- | --- | --- |
| Windows x64 | Windows 版 `adb.exe`、所需 DLL、APK 工具和 x64 Java 21 | 主程序旁的 `tools` 目录 |
| macOS Intel x64 | macOS 版 `adb`、APK 工具和 x64 Java 21 | `.app/Contents/Resources/tools` |
| macOS Apple Silicon arm64 | macOS 版 `adb`、APK 工具和 arm64 Java 21 | `.app/Contents/Resources/tools` |

Mac 包不能装入 Windows 的 EXE 或 DLL。官方 macOS ADB 从 32.0.0 起提供 Universal 通用文件，同时支持 Intel 和 Apple Silicon；本项目使用 35 及以上版本，不另维护两套 ADB，打包检查它包含当前目标架构。若 `aapt`（APK 信息读取工具）只有 Intel 版本，Apple Silicon 的构建机和实际使用电脑都需要 Rosetta；只在 CI 安装 Rosetta 不能消除用户电脑的这一要求。Android SDK 按系统提供下载，准备方式见 [Platform-Tools 官方说明](https://developer.android.com/tools/releases/platform-tools)和 [sdkmanager 官方说明](https://developer.android.com/tools/sdkmanager)。

Windows 内部发布和多平台压缩包流程都会准备并检查工具，缺少工具时发布失败。`build:windows` 只编译主程序，完整 Windows 交付仍走内部发布脚本；`build:macos` 已包括工具准备、应用构建、工具装入、签名和最终检查，输出完整 `.app`。Windows 已验证随包工具独立运行；macOS 仍需在 Intel 和 Apple Silicon 的 Mac 上分别验收。

## 智能屏桌面交互原型

在项目根目录执行以下命令，使用真实桌面窗口核对智能屏界面与交互：

```powershell
pnpm dev:desktop:screen
```

也可执行 `npm run dev:desktop:screen`。首次使用需先安装项目依赖，并具备 Tauri 桌面开发所需的 Rust 和 Windows 构建环境。

- 自动启动 1421 端口的前端开发服务和 Tauri 开发窗口，默认打开“智能屏列表”；项目、智能屏、一体机和任务等业务均使用模拟数据，不连接真实项目平台或设备。
- 使用独立应用标识 `com.inxaiot.desk-buddy.screenprototype`，Windows 默认数据目录为 `%APPDATA%\com.inxaiot.desk-buddy.screenprototype`，不复用正式工作台的数据目录。此目录仅供原型开发，不导入正式数据或数据目录切换配置。
- 修改 Vue、CSS、文案和前端交互后，通过 Vite 热更新查看结果，无需重新打包；Rust 修改仍由 Tauri 重新编译并重启桌面程序。
- 正式生产构建和下方普通 Tauri 开发模式均不开放智能屏原型。具体交互和验证边界见[智能屏原型使用与验证](doc/智能屏/04-交互原型使用与验证.md)。
- 本节专用原型只用于演示资料、检查预览、结果及冲突处理，不连接真实设备或项目数据库；真实业务使用上方普通桌面模式。

## 浏览器预览

浏览器开发预览使用 1420 端口，智能屏页面可通过[智能屏列表](http://127.0.0.1:1420/#/screen/nodes)打开。支持列表筛选、未注册智能屏管理、已注册资料草稿、注册更新、疑似合并、批量操作、状态覆盖及任务日志；所有屏数据和执行均为模拟，不连接真实设备或数据库。

```powershell
pnpm install
pnpm dev
```

打开 `http://127.0.0.1:1420/`。浏览器和智能屏桌面原型均使用开发期模拟适配器，分别保留各自的模拟记录；真实业务回归使用下方普通 Tauri 开发模式。

## Tauri开发模式与人工界面优化

在项目根目录执行：

```powershell
pnpm tauri dev
```

- 自动启动本机Vite服务及Rust Debug桌面程序，连接真实后端；使用当前Windows用户既有项目、数据目录和系统凭据。启动前正常退出占用同一数据目录的正式版。
- 修改Vue、CSS、文案和前端交互后，通过Vite热更新或页面刷新查看结果，无需重新打包EXE；首次启动仍需编译当前Rust Debug程序。
- 修改Rust后由Tauri增量编译并重启应用；这类修改安排在没有活动部署任务时，避免重启打断执行。平台登录认证由用户本人完成。
- 日常按修改范围做必要的类型检查、组件或Rust针对性回归，不逐项运行全量E2E，也不重复清空环境。
- 需要正式发布时，再执行Release优化编译及既有正式产物校验。Cargo现有Release优化设置保持不变，开发模式不使用`--release`。

## 免安装编译与正式交付

- Windows：`pnpm build:windows`生成`src-tauri/target/release/inxaiot-desk-buddy.exe`，直接运行，不生成安装器；系统需要WebView2运行时。
- macOS：在对应架构的 Mac 上准备 macOS Android SDK 和匹配架构的 Java 21，再执行 `pnpm build:macos`；输出带工具的完整 `.app`，整体复制到“应用程序”目录。该命令不支持在 Windows 打 Mac 包，也不支持在另一架构的 Mac 上交叉构建。
- Linux：在Linux上执行`pnpm build:linux`生成当前架构的可执行文件。需要按Tauri要求安装WebKitGTK等系统依赖，并提供兼容`org.freedesktop.secrets`的桌面凭据服务用于保存项目数据库密码；在目标发行版原生验证。
- 正式Windows产物：使用`scripts/release-internal.ps1`从干净提交构建，再用`scripts/verify-internal-release.ps1`校验。完整说明见[发布与回滚](doc/16-Windows可信发布与回滚说明.md)。
- 发布参数接受已有RSA、Ed25519、ECDSA私钥内容，继续加密存入工作台库；RSA认证使用SHA-2，无需追加新公钥。
- SSH主机指纹在预检和执行时自动记录，变化只告警并继续，不需要逐台采集或确认；账号认证失败等其他错误仍正常阻断。

### GitHub多平台发布

- 推送`v*.*.*`标签会触发`.github/workflows/portable-release.yml`；标签必须与`package.json`、Cargo和Tauri中的应用版本一致，当前版本使用`v0.2.5`。
- Actions并行构建Windows x64、Linux x64、macOS Intel x64和Apple Silicon arm64，生成便携压缩包及对应SHA-256文件；全部构建成功后才创建GitHub Release。手动触发只保存7天的Workflow Artifact，不创建Release。
- macOS 流水线与本机构建共用 `build:macos`，按 x64、arm64 分别准备 SDK 和 Java。默认产物使用 adhoc 签名；没有 Apple Developer 证书和公证，首次从网络下载后仍可能需要用户在系统“隐私与安全性”中允许打开。Apple Silicon 使用 Intel 版随包 Android 工具时，用户电脑也必须具备 Rosetta。
- GitHub产物用于跨平台构建和原生验收，不能替代目标Mac和Linux发行版上的实际运行、Keychain/Secret Service、文件对话框及SSH/SFTP验证。

## 质量检查

`pnpm typecheck` 和 `pnpm build` 会实际检查应用和构建配置两个 TypeScript 工程，包括组件及测试代码。`pnpm test` 先运行前端测试，再运行随包工具和 Mac 构建编排的 Node.js 测试；Mac 编排使用模拟文件，不代表已在 Mac 设备上验收。

一体机列表的“最近服务检查”只读取当前电脑的真实检查快照；历史记录不因超过15分钟变成告警。详情中的“检查服务”会执行只读SSH采集并在本机任务与日志中反馈，普通刷新不会访问一体机。平台共享已部署版本，实际状态、镜像ID、检查时间和失败原因保存在本机`local_aio_service_check`，换电脑后按需重新检查。部署健康步骤复用同次观测，单服升级不会更新其他服务检查时间。

```powershell
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo check --manifest-path .\src-tauri\Cargo.toml
pnpm tauri build --no-bundle
```
