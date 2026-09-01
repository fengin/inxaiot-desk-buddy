# Windows可信发布与回滚说明

## 1. 适用范围与当前状态

本文定义`inxaiot-desk-buddy`的P1-20发布边界。源码、构建、私钥和正式产物均保留在公司受控Windows主机，不使用GitHub托管Runner或第三方制品上传。

当前已完成：

- `Jenkinsfile.windows`分层定义能力门禁、显式真实环境门禁和签名发布门禁；Runner标签固定为`inxaiot-windows-release`并禁止并发发布。
- `src-tauri/tauri.release.conf.json`生成Current User模式NSIS安装包，允许从保留的旧签名版本受控回滚。
- `scripts/generate-sbom.ps1`生成CycloneDX 1.5 SBOM；本机实测包含905个Cargo/npm组件。
- `scripts/sign-windows.ps1`要求显式40位证书指纹、Code Signing EKU、有效私钥、有效信任链和RFC 3161时间戳，拒绝签署`src-tauri/target`之外的文件。
- `scripts/release-windows.ps1`要求Git工作区干净，以版本+12位提交ID创建不可覆盖产物目录，输出签名安装包、签名裸程序、SBOM、证书公钥、SHA-256、提交/Tree清单和CMS分离签名。
- `scripts/verify-release.ps1`离线复验CMS、证书链、清单文件大小/SHA-256和安装包/裸程序Authenticode签名。
- Tauri/NSIS未签名冒烟构建已通过，生成包被明确识别为`NotSigned`并在记录大小/SHA-256后删除；该证据只证明安装包工具链，不构成发布产物。

当前状态：内部证书与CurrentUser信任已经创建，最终Thumbprint为`4B6FA6B7CBF774B4BB0BFACEE8EC51EE8A7FC3C1`，私钥为持久非导出CNG密钥；ACL产物根已建立，手工Authenticode与DigiCert RFC3161时间戳验证为Valid。Tauri正式NSIS尚未形成，原生签名剩SignTool PATH集成。鉴于产品仅内部使用，签名是否继续作为阶段8硬门禁需用户确认；取消时应按第4节撤销证书并保留SHA-256/提交清单方案。

## 2. 指定基础设施

### 2.1 可信CI

- 使用公司现有内网Jenkins Controller。
- Windows Runner标签：`inxaiot-windows-release`。
- Runner必须使用专用Windows身份，禁止交互登录共享使用；该身份独占Code Signing私钥和`D:\inxaiot-release-artifacts`写权限。
- Jenkins凭据ID：
  - `inxaiot-windows-signing-thumbprint`：只保存证书SHA-1指纹，不保存/导出私钥。
  - `inxaiot-stage75-ed25519`：受控真实环境门禁专用私钥文件；对应公钥须由环境管理员独立安装和撤销。
- `RUN_REAL_GATES`和`PUBLISH_SIGNED_RELEASE`均为默认关闭的显式参数；真实节点副作用与正式发布不能由普通提交自动触发。

### 2.2 签名证书

计划激活的内部证书：

- Subject：`CN=INXVision IoT Desk Buddy Internal Release, O=INXVision`
- 类型：RSA-3072、SHA-256、Code Signing EKU。
- 私钥：`Cert:\CurrentUser\My`，NonExportable，仅发布Runner身份可用。
- 内部信任：证书公钥加入同一身份的`Cert:\CurrentUser\Root`和`Cert:\CurrentUser\TrustedPublisher`；目标安装电脑必须由IT以同一公钥建立信任，否则Windows会正确显示未知发布者。
- 时间戳：DigiCert RFC 3161 `http://timestamp.digicert.com`，签名和时间戳摘要均为SHA-256。
- 有效期：3年；到期前90天由发布管理员换证，新旧公钥在回滚窗口内并存。

这是内部自签名信任方案，不等同于面向公网分发的CA/EV代码签名证书。若未来面向外部客户分发，必须替换为组织主体的公有CA代码签名证书，脚本和产物格式不变。

### 2.3 产物基础设施

- 根目录：`D:\inxaiot-release-artifacts`，必须由管理员预先创建，位于源码仓库之外且不得为联接/符号链接；发布脚本不会自行创建或放宽ACL。
- ACL只允许发布Runner身份、`SYSTEM`和本机`Administrators`完全控制，移除继承权限。
- 发布前脚本按SID拒绝Everyone、Authenticated Users或Builtin Users的写入/修改/完全控制权限。
- 每个目录名固定为`inxaiot-desk-buddy-{version}-{commit12}`，存在时发布脚本拒绝覆盖。
- 产物文件发布后设置只读属性；旧版本不自动删除，作为回滚源。
- 目录仍需纳入公司备份和异机复制。单机D盘不是最终灾备，P1-20关闭证据只证明构建、签名、追溯和不可覆盖契约。

## 3. 发布、升级与回滚

正式发布必须从干净提交执行：

```powershell
$env:INX_SIGN_CERT_THUMBPRINT = "由Jenkins凭据注入"
$env:INX_RELEASE_ARTIFACT_ROOT = "D:\inxaiot-release-artifacts"
.\scripts\release-windows.ps1
```

安装或回滚前先验证目标产物集：

```powershell
.\scripts\verify-release.ps1 -ArtifactSet "D:\inxaiot-release-artifacts\inxaiot-desk-buddy-版本-提交"
```

升级使用新目录中的`*-setup.exe`。回滚只能选择已经验证通过的旧不可覆盖目录，再运行旧签名安装包；不得从`target/`、聊天附件或临时共享目录直接安装。

应用数据目录不随安装包卸载。升级/回滚前必须完成工作台数据目录备份；若新版本包含不可逆Schema迁移，必须在版本说明中阻断直接降级并提供数据回滚步骤。

## 4. 精确撤销方案

如内部签名私钥疑似泄漏，以记录的唯一Thumbprint为条件执行：

1. 停止Jenkins发布Job并撤销`inxaiot-windows-signing-thumbprint`凭据。
2. 从`Cert:\CurrentUser\My`删除带私钥证书。
3. 从`Cert:\CurrentUser\Root`和`Cert:\CurrentUser\TrustedPublisher`删除同Thumbprint公钥。
4. 在全部目标电脑撤销同公钥信任。
5. 保留既有产物集作为事件证据但停止分发，使用新证书重新构建新提交；禁止覆盖旧目录。

删除必须同时校验Subject和Thumbprint，不能按模糊名称批量删除证书。

## 5. 首次关闭门禁

只有以下证据同时存在，才允许在doc/05、doc/14和doc/15中关闭P1-20：

1. 指定证书在My/Root/TrustedPublisher中的Subject和Thumbprint一致，私钥不可导出，证书链有效。
2. 干净提交执行完整`quality-gate.ps1`成功。
3. NSIS安装包和裸程序`Get-AuthenticodeSignature`均为`Valid`，签名Thumbprint与清单一致，并含有效RFC 3161时间戳。
4. CycloneDX、release-manifest、CMS签名及checksums齐全，`verify-release.ps1`通过。
5. 产物目录名中的提交与应用关于页构建提交、release-manifest提交完全一致。
6. 使用安装包完成一次安装/升级，并从保留旧签名产物完成一次受控回滚；应用数据目录和数据库事实保持可读。
