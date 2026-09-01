# Windows内部发布与回滚说明

## 1. 适用范围与当前状态

本文定义`inxaiot-desk-buddy`的P1-20内部发布边界。源码、构建和正式产物均保留在公司受控Windows主机，不使用GitHub托管Runner或第三方制品上传。产品仅在内部受控环境分发，2026-09-01经用户确认取消代码签名硬门禁。

当前已完成：

- `Jenkinsfile.windows`分层定义能力门禁、显式真实环境门禁和内部发布门禁；Runner标签固定为`inxaiot-windows-release`并禁止并发发布。
- `src-tauri/tauri.release.conf.json`生成Current User模式无签名NSIS安装包，允许从保留的旧版本受控回滚。
- `scripts/generate-sbom.ps1`生成CycloneDX 1.5 SBOM；本机实测包含905个Cargo/npm组件。
- `scripts/release-internal.ps1`要求Git工作区干净，以版本+12位提交ID创建不可覆盖产物目录，构建后校验Cargo Release依赖元数据绑定同一非dirty提交，并复查工作区、HEAD和Tree未在构建期间变化，再输出无签名安装包、无签名裸程序、SBOM、SHA-256及提交/Tree清单；最终发布采用同卷原子移动。
- `scripts/verify-internal-release.ps1`离线复验文件集合、大小/SHA-256、Git提交/Tree、目录命名、只读属性和安装包/裸程序`NotSigned`状态；`release-windows.ps1`与`verify-release.ps1`仅作为兼容入口委托新脚本。
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
- Windows会把安装包显示为未知发布者，这是当前内部产品边界下明确接受的体验，不应伪装为已签名。
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

安装或回滚前先验证目标产物集：

```powershell
.\scripts\verify-internal-release.ps1 -ArtifactSet "D:\inxaiot-release-artifacts\inxaiot-desk-buddy-版本-提交"
```

升级使用新目录中的`*-setup.exe`。回滚只能选择已经验证通过的旧不可覆盖目录，再运行旧安装包；不得从`target/`、聊天附件或临时共享目录直接安装。安装前必须在受控源码副本中执行校验脚本并核对提交。

应用数据目录不随安装包卸载。升级/回滚前必须完成工作台数据目录备份；若新版本包含不可逆Schema迁移，必须在版本说明中阻断直接降级并提供数据回滚步骤。

P1-20在本轮关闭的是“可重复构建、完整性、来源和不可覆盖发布”门禁；实际安装、升级、卸载与旧版本回滚仍属于阶段8用户验收，不因取消签名而被豁免。

## 4. 历史签名资产清理结果

2026-09-01按用户授权，以Subject和唯一Thumbprint双重确认后完成：

1. `Cert:\CurrentUser\My`中该证书为0。
2. `Cert:\CurrentUser\Root`中该公钥为0。
3. `Cert:\CurrentUser\TrustedPublisher`中该公钥为0。
4. 已知CNG KeyName、UniqueName私钥文件及`certutil -user -key`匹配数均为0。
5. `D:\inxaiot-release-artifacts`及其受控ACL保留，不删除已验收的内部产物集。

禁止按模糊Subject批量删除其他证书。历史签名实现和专用签名脚本已删除；`release-windows.ps1`与`verify-release.ps1`仅保留无签名链兼容转发。

## 5. 首次关闭门禁

只有以下证据同时存在，才允许在doc/05、doc/14和doc/15中关闭P1-20：

1. 干净提交执行完整`quality-gate.ps1`成功。
2. Tauri生成Current User模式NSIS安装包和裸程序，二者`Get-AuthenticodeSignature`均明确为`NotSigned`。
3. CycloneDX、release-manifest和checksums齐全，清单记录版本、完整提交、Git Tree、文件大小及SHA-256。
4. 产物目录不存在时才允许发布，所有文件设置只读，失败只清理本次唯一staging目录。
5. 发布时Cargo Release依赖元数据绑定12位提交且无dirty；`verify-internal-release.ps1`复验文件集合、角色唯一性、哈希、Git来源和无签名策略通过，兼容入口结果一致。
6. 产物根ACL无Everyone、Authenticated Users或Builtin Users写权限。

首次关闭实证：产物集`inxaiot-desk-buddy-0.1.0-f9adb6ec6845`、提交`f9adb6ec6845b8e5267a9bf7a19551b6eb8245fc`、Tree `45e4aa36a36e0f1389379449e47da24b357c3394`；安装包SHA-256 `f7d2c2a4f355862caa4ab2868c4c7ac1c62c0a34ea279b8c353dc76974134da4`，裸程序SHA-256 `d51c613f1feccbfdf921ac5d812d66c0dbd02775f1c5fe2197127881885c9dd9`，staging残留0。
