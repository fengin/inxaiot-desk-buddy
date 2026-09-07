# INX 实施工作台（重构版）

面向项目实施与维护人员的桌面工作台，根据 `inxaiot-edge-workbench` 的实际能力重新进行产品、技术和工程设计。

阶段8三类双节点正式桌面E2E及独立事实核验已完成，当前由用户人工操作并反馈界面、文案、交互和细节功能问题，逐项优化并做针对性回归。环境清空与保留范围见[阶段8记录](doc/17-阶段8正式桌面E2E验收记录.md)第15节；新旧版使用由用户决定，不作为验收门禁，旧版代码保留。

当前部署物模型以项目共享配置为中心：`.env`、`docker-compose.yml`、`host-info.json`三个模板及发布参数保存到项目`inxaiot_desk_buddy`，Compose决定可部署服务。首次部署和整包升级不再选择Release目录或要求用户准备`manifest.json`，只为Compose服务选择单镜像tar并确认RepoTag；工作台在上传前完成模板、YAML、端口、镜像和逐节点渲染校验，再自动生成Agent使用的内部发布包。

## 文档

- [开发阶段总结与代码 Review 交接（2026-09-06）](doc/18-开发阶段总结与代码Review交接.md)
- [重构背景](doc/重构背景.md)
- [桌面工作台界面设计规范](doc/01-桌面工作台界面设计规范.md)
- [产品设计文档](doc/02-产品设计文档.md)
- [技术方案设计](doc/03-技术方案设计.md)
- [界面Demo验收说明](doc/04-界面Demo验收说明.md)
- [开发实施与验收记录](doc/05-开发实施与验收记录.md)
- [阶段7多实例、恢复、性能与真实环境验收记录](doc/13-阶段7多实例恢复性能与真实环境验收记录.md)
- [架构与产品闭环Review问题及整改计划](doc/14-架构与产品闭环Review问题及整改计划.md)

## 浏览器预览

```powershell
pnpm install
pnpm dev
```

打开 `http://127.0.0.1:1420/`。此浏览器入口使用开发期Fixture，仅用于独立界面预览；人工业务回归使用下方Tauri开发模式。

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
- macOS：在Mac上执行`pnpm build:macos`，将输出的完整`.app`复制到“应用程序”目录。Windows不能替代Mac原生构建与运行验证。
- Linux：在Linux上执行`pnpm build:linux`生成当前架构的可执行文件。需要按Tauri要求安装WebKitGTK等系统依赖，并提供兼容`org.freedesktop.secrets`的桌面凭据服务用于保存项目数据库密码；在目标发行版原生验证。
- 正式Windows产物：使用`scripts/release-internal.ps1`从干净提交构建，再用`scripts/verify-internal-release.ps1`校验。完整说明见[发布与回滚](doc/16-Windows可信发布与回滚说明.md)。
- 发布参数接受已有RSA、Ed25519、ECDSA私钥内容，继续加密存入工作台库；RSA认证使用SHA-2，无需追加新公钥。
- SSH主机指纹在预检和执行时自动记录，变化只告警并继续，不需要逐台采集或确认；账号认证失败等其他错误仍正常阻断。

## 质量检查

一体机列表的“最近服务检查”只读取当前电脑的真实检查快照；历史记录不因超过15分钟变成告警。详情中的“检查服务”会执行只读SSH采集并在本机任务与日志中反馈，普通刷新不会访问一体机。平台共享已部署版本，实际状态、镜像ID、检查时间和失败原因保存在本机`local_aio_service_check`，换电脑后按需重新检查。部署健康步骤复用同次观测，单服升级不会更新其他服务检查时间。

```powershell
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo check --manifest-path .\src-tauri\Cargo.toml
pnpm tauri build --no-bundle
```
