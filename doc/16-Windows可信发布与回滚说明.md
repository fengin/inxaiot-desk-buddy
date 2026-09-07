# Windows免安装发布、macOS应用包与回滚说明

## 1. 适用范围与当前状态

本文定义`inxaiot-desk-buddy`的内部发布边界。源码、构建和正式产物留在公司受控主机，不使用GitHub托管Runner或第三方制品上传。2026-09-01用户确认取消内部Windows代码签名硬门禁；2026-09-02进一步确认Windows免安装EXE、macOS可复制`.app`。NSIS安装不再是交付或阶段8的前置要求。

当前已完成：

- `Jenkinsfile.windows`分层定义能力门禁、显式真实环境门禁和内部发布门禁；Runner标签固定为`inxaiot-windows-release`并禁止并发发布。
- `src-tauri/tauri.release.conf.json`关闭安装器打包；`--no-bundle`只是不生成安装包，仍是优化后的正式Release EXE，不等于debug或desktop-e2e构建。
- `scripts/generate-sbom.ps1`生成CycloneDX 1.5 SBOM，组件数量以本次清单为准。
- `scripts/release-internal.ps1`要求Git工作区干净，以版本+12位提交ID创建不可覆盖产物目录，构建后校验Cargo Release依赖元数据绑定同一非dirty提交，并复查工作区、HEAD和Tree未变化；输出免安装EXE、SBOM、SHA-256及清单v3。最终发布采用同卷原子移动。
- `scripts/verify-internal-release.ps1`离线复验文件集合、大小/SHA-256、Git提交/Tree、目录命名、只读属性和EXE的`NotSigned`状态。v3必须包含portable与sbom各一份；兼容历史v2的portable/installer/sbom，不修改旧产物。旧脚本名继续转发。
- 首个正式内部产物集已由干净提交`f9adb6ec6845b8e5267a9bf7a19551b6eb8245fc`生成并通过两次离线复验。

当前状态：P1-20按内部无签名发布策略关闭。历史内部证书Thumbprint `4B6FA6B7CBF774B4BB0BFACEE8EC51EE8A7FC3C1`已从CurrentUser My/Root/TrustedPublisher精确删除，已知CNG容器和私钥文件匹配数均为0；受控产物根继续保留。

## 2. 指定基础设施

### 2.1 可信CI

- 使用公司现有内网Jenkins Controller。
- Windows Runner标签：`inxaiot-windows-release`。
- Runner必须使用专用Windows身份，禁止交互登录共享使用；该身份独占`D:\inxaiot-release-artifacts`写权限。
- Jenkins凭据`inxaiot-stage75-ed25519`仅供受控真实环境门禁使用；对应公钥须由环境管理员独立安装和撤销，内部发布本身不读取该凭据。
- `RUN_REAL_GATES`和`PUBLISH_INTERNAL_RELEASE`均为默认关闭的显式参数；真实节点副作用与正式发布不能由普通提交自动触发。

### 2.2 无签名信任边界

- 不再创建、导入或信任内部代码签名证书，CI不需要证书指纹、私钥或时间戳服务。
- Windows可能对无签名EXE显示未知发布者或SmartScreen提示；免安装不绕过系统安全检查，不应伪装为已签名。
- 完整性依赖受控NTFS ACL、不可覆盖目录、Git提交/Tree、release-manifest和SHA-256共同建立；哈希不能替代访问控制，访问控制也不能替代来源追溯。
- 若未来转为外部分发、域级软件分发或安全基线要求可信发布者，必须单独立项启用组织代码签名证书并重做安装/升级/回滚门禁，不得静默恢复历史自签名方案。

### 2.3 产物基础设施

- 根目录：`D:\inxaiot-release-artifacts`，必须由管理员预先创建，位于源码仓库之外且不得为联接/符号链接；发布脚本不会自行创建或放宽ACL。
- ACL只允许发布Runner身份、`SYSTEM`和本机`Administrators`完全控制，移除继承权限。
- 发布前脚本按SID拒绝Everyone、Authenticated Users或Builtin Users的写入/修改/完全控制权限。
- 每个目录名固定为`inxaiot-desk-buddy-{version}-{commit12}`，存在时发布脚本拒绝覆盖。
- 产物文件发布后设置只读属性；旧版本不自动删除，作为回滚源。
- 目录仍需纳入公司备份和异机复制。单机D盘不是最终灾备，P1-20关闭证据只证明构建、完整性校验、追溯和不可覆盖契约。

## 3. 发布、升级与回滚

正式发布必须从干净提交执行：

```powershell
$env:INX_RELEASE_ARTIFACT_ROOT = "D:\inxaiot-release-artifacts"
.\scripts\release-internal.ps1
```

运行或回滚前先验证目标产物集：

```powershell
.\scripts\verify-internal-release.ps1 -ArtifactSet "D:\inxaiot-release-artifacts\inxaiot-desk-buddy-版本-提交"
```

验证通过后，直接双击新目录中的`inxaiot-desk-buddy-版本-x64.exe`。新产物不再包含`*-setup.exe`；历史`*-setup.exe`是安装器，不是可携带主程序。Windows 10/11 x64需要Microsoft Edge WebView2 Evergreen Runtime；EXE不携带固定WebView2运行时，不承诺无运行时电脑也可直接启动。缺少时使用微软官方运行时安装流程，不能通过禁用系统安全检查解决。

升级：等待任务结束、退出旧程序、备份应用数据，再运行新版本EXE。回滚：核验旧不可覆盖目录，确认Schema向下兼容后运行旧主程序。不得覆盖正在运行的EXE，也不要把`target/`中的开发产物当作正式交付。

本地SQLite、日志和WebView数据仍在当前用户的标准应用数据目录，凭据仍在Windows Credential Manager；免安装不意味着数据与EXE同目录。移走EXE不会删除数据。不可逆Schema迁移必须阻断直接降级并提供数据恢复方案。

阶段8使用上述干净正式免安装EXE完成Computer Use；发布门禁通过不等于桌面功能验收通过。

### 3.1 日常编译入口

在项目根目录执行`pnpm build:windows`生成`src-tauri/target/release/inxaiot-desk-buddy.exe`。内部正式交付仍使用前述`release-internal.ps1`，以获得受控且可追溯的完整产物集。

### 3.2 macOS

在Mac上安装项目已有Node/pnpm、Rust和Xcode构建依赖后，于项目根目录执行`pnpm build:macos`。入口固定`--bundles app`和`tauri.macos.conf.json`，默认构建当前Mac架构，输出`src-tauri/target/release/bundle/macos/INX 实施工作台.app`。

PNG和ICNS图标已经保存在`src-tauri/icons/`，干净检出后可直接用于Mac开发和构建，不需要先执行图标生成步骤。

把整个`.app`复制到“应用程序”目录再打开，不要只复制包内的可执行文件；不需要PKG安装器。Intel与Apple Silicon需分别构建/验证，不将单架构产物宣称为通用包。macOS凭据使用系统Keychain，普通应用数据不写入`.app`。

当前Windows主机只完成配置与入口检查，未生成或实际验证Mac产物。Mac正式发布还必须记录干净提交、产物完整性和原生GUI验收。通过下载渠道分发时Gatekeeper仍可能要求可信签名/公证；“复制即可用”描述包的使用方式，不代表绕过系统信任策略。参考[Tauri应用包说明](https://v2.tauri.app/distribute/macos-application-bundle/)与[Windows运行时说明](https://v2.tauri.app/distribute/windows-installer/)。

### 3.3 Linux

在Linux上安装项目已有Node/pnpm、Rust及Tauri要求的WebKitGTK等系统依赖后，执行`pnpm build:linux`生成当前架构的可执行文件。该入口会拒绝在非Linux系统运行，避免在Windows上误把本机产物当作Linux产物。Linux桌面运行、系统凭据、文件对话框和脚本查看仍需在目标发行版原生验收。

## 4. 历史签名资产清理结果

2026-09-01按用户授权，以Subject和唯一Thumbprint双重确认后完成：

1. `Cert:\CurrentUser\My`中该证书为0。
2. `Cert:\CurrentUser\Root`中该公钥为0。
3. `Cert:\CurrentUser\TrustedPublisher`中该公钥为0。
4. 已知CNG KeyName、UniqueName私钥文件及`certutil -user -key`匹配数均为0。
5. `D:\inxaiot-release-artifacts`及其受控ACL保留，不删除已验收的内部产物集。

禁止按模糊Subject批量删除其他证书。历史签名实现和专用签名脚本已删除；`release-windows.ps1`与`verify-release.ps1`仅保留无签名链兼容转发。

## 5. 首次关闭门禁（历史NSIS基线）

本节保留2026-09-01已执行的历史事实，不作为2026-09-02以后新产物的NSIS要求。新门禁采用第1—3节免安装策略。

只有以下证据同时存在，才允许在doc/05、doc/14和doc/15中关闭P1-20：

1. 干净提交执行完整`quality-gate.ps1`成功。
2. Tauri生成Current User模式NSIS安装包和裸程序，二者`Get-AuthenticodeSignature`均明确为`NotSigned`。
3. CycloneDX、release-manifest和checksums齐全，清单记录版本、完整提交、Git Tree、文件大小及SHA-256。
4. 产物目录不存在时才允许发布，所有文件设置只读，失败只清理本次唯一staging目录。
5. 发布时Cargo Release依赖元数据绑定12位提交且无dirty；`verify-internal-release.ps1`复验文件集合、角色唯一性、哈希、Git来源和无签名策略通过，兼容入口结果一致。
6. 产物根ACL无Everyone、Authenticated Users或Builtin Users写权限。

首次关闭实证：产物集`inxaiot-desk-buddy-0.1.0-f9adb6ec6845`、提交`f9adb6ec6845b8e5267a9bf7a19551b6eb8245fc`、Tree `45e4aa36a36e0f1389379449e47da24b357c3394`；安装包SHA-256 `f7d2c2a4f355862caa4ab2868c4c7ac1c62c0a34ea279b8c353dc76974134da4`，裸程序SHA-256 `d51c613f1feccbfdf921ac5d812d66c0dbd02775f1c5fe2197127881885c9dd9`，staging残留0。
