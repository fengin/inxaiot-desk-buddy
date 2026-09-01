# INX 实施工作台全仓代码与产品深度 Review 分析报告

| 属性 | 内容 |
| --- | --- |
| Review 日期 | 2026-08-31 |
| 适用项目 | `inxaiot-desk-buddy` |
| Review 角色 | 架构师、产品负责人、实施用户、安全与运维视角 |
| Review 范围 | Rust/Tauri、Vue/Pinia、SQLite/MySQL、平台认证、SSH/SFTP、Shell Agent、Release/模板、三类部署、任务/恢复、数据目录、日志/脱敏、测试、依赖与发布 |
| 代码规模 | 283个正式文件；147个Rust、68个TypeScript、8个Vue、4个SQL；54个测试相关文件 |
| IPC与测试 | 整改后46个Tauri Command、46个shared/api invoke点；17个前端Spec、38个Rust集成测试文件、36个真实环境ignore测试 |
| 当前结论 | P0-06/P0-07/P0-09、P1-08及Ed25519双节点主链已关闭；P0-11故障回滚分支通过但暴露rule-engine旧SQLite迁移缺口，开发MySQL TLS、正式桌面故障和受信发布仍阻断阶段8 |

## 1. 执行摘要

本次不是只复核阶段7.5，也不是围绕已知问题做定向扫描，而是从产品入口、IPC、应用用例、数据库、远端执行到用户结果页面逐链路检查整个工程。

已确认的正向基础：

- Tauri Real Adapter与浏览器Fixture的主入口已物理区分，生产页面和Store没有直接`invoke()`。
- 平台数据库连接设置为只读会话，当前平台Repository正式语句只有SELECT。
- TaskQueue、JobSupervisor、CancellationToken和ExecutionCoordinator已经形成可复用骨架。
- HostKey固定、SFTP分块传输、Agent action白名单、Release相对路径校验和本地敏感值存储方向正确。
- 当前已有自动化与真实环境测试能够证明已覆盖的正常路径和部分失败路径真实可运行。

但“测试通过”不能证明未覆盖的架构不变量。全仓深审确认：

- 15组P0阻断根因，必须在阶段8前修复。
- 20组P1高优先级问题，原则上在替换原工作台前清零，其中安全、真实状态和规模能力相关项应随P0一起修。
- 8组P2工程与体验问题，可在不影响安全和数据一致性的前提下排入后续。

因此，`doc/05`与`doc/14`中“阶段7.5已完成”的结论不再作为阶段8准入依据。本报告形成新的唯一问题基线。

## 2. Review方法与证据

### 2.1 检查方法

1. 盘点全部正式文件、模块、Command、前端invoke、Store、Adapter、Migration和测试。
2. 从用户操作反向追踪页面 → Store → Real Adapter → Command → 应用/端口 → Repository/外部系统。
3. 从数据一致性正向追踪SQLite、OS SecretStore、工作台MySQL、平台只读库、远端文件和Docker状态。
4. 对任务提交、排队、取消、崩溃、心跳、租约接管、最终化、退出和重启做状态机交叉检查。
5. 对Release指纹、模板渲染、上传、Agent执行、备份、升级、健康检查和清理做故障原子性检查。
6. 对项目切换、会话过期、跨项目异步响应、分页规模、错误态和用户可恢复入口做产品检查。
7. 交叉核对产品/技术文档承诺与当前实现。
8. 执行依赖安全审计和构建发布边界检查。

### 2.2 自动化与依赖证据

- 前端官方npm生产依赖审计：182项生产/可选依赖，critical/high/moderate/low均为0。
- RustSec审计数据库更新时间为2026-08-29，当前Cargo.lock共742项依赖；发现3条漏洞记录：
  - `RUSTSEC-2026-0185`：`quinn-proto 0.11.14`，Windows目标依赖树不存在，不影响当前Windows Release。
  - `RUSTSEC-2023-0071`：`rsa 0.9.10`和`rsa 0.10.0-rc.18`，Windows目标存在。平台登录当前只做RSA公钥加密，不执行私钥操作；但russh的RSA私钥认证路径属于风险适用范围，当前开发测试私钥也是RSA 4096。
  - `chacha20 0.10.1`为yanked版本，经russh进入Windows依赖树，需跟随russh升级收敛。
- Linux GTK3未维护/unsound告警不进入当前Windows目标树。
- 项目没有CI配置；真实数据库/SSH/Docker门禁依赖35个`#[ignore]`测试和人工命令。
- 工程自身没有独立Git仓库，并被根元仓库`.gitignore`忽略，当前代码、文档和后续修复没有可审计版本基线。

## 3. 架构与功能现状图

```text
Vue Page
  → Pinia Store
  → Real Adapter / shared api
  → Tauri Command
  → Application Use Case / Port（部分）
  → Formal/Infrastructure Repository
  → SQLite / Workbench MySQL / Platform MySQL RO / HTTP / SSH / SFTP / Agent

生产任务
  → submit_deployment
  → 本地payload
  → TaskQueue
  → JobSupervisor
  → AIO Handler
  → ExecutionCoordinator
  → Operation + Lease
  → Remote Agent
  → 项目侧最终结果
```

主要偏差：

- Application层仍有37处直接依赖formal/infrastructure或直接SQL，端口抽象只覆盖部分新用例。
- 正式IPC同时保留新生产队列和旧同步部署两条路径。
- 文档声明的“任务输入不可变、全局并发、fencing最终写、单事务最终化、最终化重试”尚未由代码证明。

## 4. P0：阶段8前必须修复

### P0-01 缺少真实版本库边界

证据：

- 项目目录和`inxvision-assistance`均无`.git`。
- 根仓库规则`/inxvision-assistance/*`忽略本工程。

影响：

- 无法形成可追溯Review基线、差异审查、回滚点和发布版本。
- 后续集中整改存在覆盖、丢失和无法证明产物来源的风险。

修复要求：

- 在任何P0代码修改前明确目标Git仓库，保存当前完整基线并记录分支/提交。
- Release必须可以追溯到提交ID、依赖锁文件和构建参数。

验收：

- 独立仓库`git status`可见全部文件；干净提交可完成全量构建；最终程序显示或导出commit标识。

### P0-02 同一数据目录没有进程独占锁

证据：

- 启动直接解析数据目录、打开SQLite并调用`recover_interrupted()`：`src-tauri/src/lib.rs:98-118`。
- `recover_interrupted()`无实例ID/心跳判断，直接把queued/running/cancelling/finalizing_failed改为interrupted：`task_repository.rs:439-493`。
- 全仓没有single-instance或data-root lock实现。

影响：

- 用户双击启动第二进程时，第二进程会把第一进程仍在执行的任务标成中断，第一进程却可能继续远端Docker操作。
- 两个进程还能同时写SQLite、任务日志、数据目录选择器和OS凭据。

修复要求：

- SQLite打开前获取“实际数据根目录”级Windows独占锁。
- 锁被占用时激活已有窗口或明确阻止第二实例。
- 恢复只允许在确认旧锁所有者死亡后执行。

验收：

- 两进程同目录真实门禁；第二进程不能修改任何Task/Target/Step。

### P0-03 旧`execute_deployment`绕过生产执行内核

证据：

- Command仍直接调用`launch_deployment`：`interface/commands/release_artifacts.rs:62-69`。
- Command仍注册到正式Tauri：`src-tauri/src/lib.rs:197`。
- `OperationsAdapter.execute()`和`RealOperationsAdapter.execute()`仍公开该能力。

影响：

- 可绕过ProjectAccessGuard、TaskQueue、立即Task ID、队列容量、统一取消和HandlerRegistry。
- 同一产品存在两套正式执行语义。

修复要求：

- 删除旧Command、API和Adapter方法；所有三类部署只能进入`submit_deployment`。
- 增加正式Command allowlist测试，禁止出现第二执行入口。

### P0-04 排队任务没有不可变执行快照

证据：

- 预检报告包含`profile_version`，但写入payload时只保存`LaunchDeploymentInput { plan }`：`stage75b_submission_adapter.rs:33-80`。
- Plan只有本机路径、名称和版本，没有制品SHA-256、大小、节点资产版本、IP或HostKey指纹。
- Handler执行时重新读取当前ReleaseProfile、节点和HostKey。
- Release目录指纹已计算，却未进入Plan、payload、Agent环境或共享操作。

影响：

- 排队期间发布参数、SSH凭据、目标IP、HostKey或本地文件变化后，实际执行内容与用户确认的预检内容不同。
- 同版本不同内容无法可靠识别。

修复要求：

- payload冻结profile版本、节点资产版本/IP、HostKey算法/指纹、Agent版本/哈希、制品路径/大小/SHA-256或Release目录指纹。
- Handler在第一个远端副作用前重新校验全部快照；变化则CheckFailed且远端副作用为0。
- 大型制品使用内容寻址或任务专属只读副本，不能只引用可变原路径。

### P0-05 项目编辑/删除与跨项目页面状态未绑定任务归属

证据：

- 编辑项目先关闭ProjectRuntime，删除项目直接删除本地入口：`stage75_adapter.rs:278-310`。
- `local_task.local_project_id`为`ON DELETE CASCADE`。
- 后端没有项目级活动任务守卫。
- 部署页面没有监听项目切换；提交、轮询、取消均读取“当前项目ID”，而不是预检/任务的原始项目ID。
- Release/AIO Store没有请求序号或AbortController，旧项目响应可覆盖新项目状态。

影响：

- 删除项目会级联删除正在执行任务的本地状态，后台Job仍可能继续。
- 切换项目后可能把旧预检计划提交到新项目，或无法继续跟踪原任务。
- 快速切换可能把A项目的完整凭据显示/保存到B项目。

修复要求：

- 项目级active task guard；活动任务存在时禁止修改连接参数和删除项目。
- 所有异步Store使用projectId快照与请求代次，过期响应丢弃。
- Task页面保存originProjectId，切项目后仍按原项目跟踪；新项目不能复用旧预检。

### P0-06 同一实例可接管自己的其他操作租约

证据：

- `acquire_one`只在`owner != request.owner_instance_id`时返回Busy：`resource_lease_repository.rs:247`。
- 同一进程内所有任务共享`application_instance_id()`。

影响：

- 两个重叠任务并发通过预检后，同一实例的第二个操作会覆盖第一个操作的租约并递增fencing。
- 第一个远端任务可能已经开始，随后心跳/检查失败，形成并行副作用。

修复要求：

- 活动且未过期租约必须按`operation_id`判定；不同operation一律Busy，即使owner相同。
- 只有相同operation+明确幂等token才允许重入。

验收：

- 同实例双任务、不同实例双任务和预检竞争三类并发门禁。

### P0-07 fencing与项目侧最终化不满足设计不变量

证据：

- 最终化开始时逐条`validate_fencing`，随后分多次独立SQL写目标结果、服务版本、节点资产、操作记录和租约释放。
- `finalize_target`、`save_service_version`、`mark_operation_success`和`operation.finalize`的SQL不带fencing条件。
- 操作记录最终化成功后才逐条释放租约；释放失败会把本地Task标为finalizing_failed，但共享操作已经终态。
- 代码没有最终化重试入口，却在finalizing_failed后删除任务payload/制品。

影响：

- 校验与写入之间租约可能过期/被接管，旧实例仍能写最终资产，fencing失效。
- 中途失败会产生部分目标、部分资产、终态Operation和活动Lease混合状态，无法按当前接口幂等重试。

修复要求：

- 建立单个工作台MySQL最终化事务：锁定并验证全部lease/fencing，幂等写目标结果和资产，最终化Operation并释放lease。
- 定义可重复执行的finalization snapshot；重试只补写项目数据库，禁止重放远端步骤。
- finalizing_failed完成前保留必要快照并提供用户入口。

### P0-08 TaskQueue异常结果没有统一持久化收敛

证据：

- Handler正常返回错误时内部尝试收敛；但Handler panic、缺失Handler、Supervisor spawn失败只生成QueueResult。
- 生产没有持续消费QueueResult并把异常结果写回Task/Target。
- 排队到Supervisor注册之间，取消只yield四次，仍可能返回NotFound。

影响：

- panic或调度异常时任务可长期停留Queued/Running，直到下次启动才被粗暴标Interrupted。
- 调度边界取消可能对用户报失败，而任务随后实际执行。

修复要求：

- QueueResult进入统一TaskOutcomeReconciler，覆盖Completed/Failed/Panicked/Aborted/MissingHandler。
- 增加Dispatching状态或原子队列所有权，取消不依赖四次yield猜测。

### P0-09 Release版本可穿越远端部署根目录

证据：

- `manifest.version`只检查非空：`domain/aio/release.rs:227-250`。
- Agent直接构造`$DEPLOY_ROOT/releases/$RELEASE_VERSION`并执行`rm -rf`：`edge-node-agent.sh:304-307`。

影响：

- 包版本包含`../`、斜杠或特殊路径时可能删除/覆盖部署根目录外内容。

修复要求：

- Rust领域层和Agent双重校验Release版本为安全单段标识。
- Agent对所有派生路径做canonical boundary检查，禁止依赖上游单点校验。

### P0-10 用户编辑的Compose模板未进入实际部署

证据：

- 发布参数页允许编辑并保存`composeTemplate`。
- 生产准备阶段用该模板生成`compose_preview`，但只写入`.env`和`host-info.json`，丢弃渲染后的Compose。
- Agent最终使用Release tar内部原始`docker-compose.yml`。

影响：

- 页面显示“配置已保存/校验通过”，实际远端部署完全不使用用户编辑的Compose配置。

修复要求：

- 产品二选一并固定：Compose由Release包只读权威提供，删除共享编辑；或将渲染结果作为任务快照写入任务专属Release并由Agent实际使用。
- 增加“修改Compose值后远端实际Compose改变”的用户门禁。

### P0-11 Agent升级缺少故障原子性和自动回滚

证据：

- 整包安装先`rm -rf`同版本目录，再复制、停止当前服务、启动新服务；任一步失败没有恢复旧current和旧服务。
- `backup_current`大量使用`|| true`，即使没有备份成功仍发送success事件。
- 单服升级先改当前`.env`，compose/健康失败时不恢复`.env.before`和旧镜像。
- 取消只关闭SSH channel，没有远端进程组或Agent协作取消协议。

影响：

- 升级失败或取消可能留下服务停止、配置指向新镜像但容器仍旧/缺失、备份不可用等半状态。
- UI可能显示失败/取消，但远端命令继续完成Docker变更。

修复要求：

- Agent实现prepare/commit/rollback状态机和operationId幂等日志。
- 新Release先在新目录验证，失败自动重启旧Release。
- 单服失败自动恢复env并重建旧服务。
- 备份逐项校验，任何必需项失败不得报告success。
- 取消需远端可识别并可查询最终状态。

### P0-12 平台会话不是持续授权事实

证据：

- 登录后Token保存到OS SecretStore，但后续业务只读取本地用户名和expires_at，Token从未用于平台校验。
- 登录响应缺少`expires_in`时，expires_at为None并被视为永久Active。
- 前端没有周期过期检查，业务Command失败后也不会统一使菜单/路由失效。

影响：

- 用户被禁用、Token撤销或平台会话失效后，工作台仍可继续数据库和远端部署操作。
- 无过期字段的异常响应可能形成永久登录。

修复要求：

- 缺失/非法过期时间fail-closed。
- 定期或关键写操作前调用平台会话验证接口；撤销/401统一清理本地会话并广播ProjectStateChanged。
- 前端全局处理认证错误并立即关闭业务入口。

### P0-13 发布凭据密文绑定数据库密码

证据：

- `encrypt_release_credentials(database_password, ...)`直接从当前数据库密码派生AES密钥。
- 读取时用当前本地项目数据库密码解密。
- 项目编辑允许更换数据库密码，没有旧密文重加密流程。

影响：

- 数据库密码轮换后共享发布凭据永久无法解密。
- 同一工作台库使用不同数据库账号/密码的两名实施人员无法读取同一共享配置。

修复要求：

- 设计稳定项目主密钥与版本化KEK，不得直接绑定连接密码。
- 支持密钥轮换和旧格式迁移；明确跨电脑安全分发模型。
- 在新方案落地前禁止静默修改会破坏解密的数据库密码。

整改结论（检查点`9b73236`）：

- 当前格式为`argon2id-aes256gcm-project-key`；32字节随机项目主密钥按版本保存到Windows Credential Manager，MySQL只保存盐、随机数、认证密文、格式和密钥版本，数据库密码不再参与新密文派生。
- 旧`argon2id-aes256gcm`密文首次读取或数据库密码修改前在`SELECT ... FOR UPDATE`事务内迁移；轮换固定为先保存新版本密钥，再锁行解密/重加密/审计，提交失败删除新密钥并继续使用旧版本。
- `.inxkey`包使用Argon2id和AES-256-GCM；AAD绑定平台URL、数据库主机/端口、工作台Schema指纹及密钥版本。导入先校验绑定/版本并实际解密当前发布配置，成功后才写本机安全存储。
- 页面仍经Release Store→Workbench Adapter→Real Adapter→Tauri Command调用；浏览器Fixture独立实现。配置因本机缺钥读取失败时，“导入并验证密钥包”入口仍可使用，口令在弹窗关闭、项目切换或成功后清空。
- 隔离MySQL真实门禁覆盖旧密文迁移、数据库密码替换后读取、轮换审计故障全回滚、成功轮换、跨本机项目ID导入和精确清理归零；Windows Credential Manager唯一引用写/读/删及页面缺钥恢复用户测试通过。

### P0-14 数据目录和启动维护缺少崩溃恢复

证据：

- 迁移直接复制到最终目标目录，无staging/journal/文件校验/原子发布。
- 目标路径只做字符串小写比较，不能可靠识别Junction、符号链接和真实路径别名。
- schedule写配置失败后内存`pending_switch`不回滚。
- 启动配置主文件损坏时不会尝试解析备份。
- 启动清理任一制品、目录或retention sidecar失败都会通过`?`中止整个Tauri setup。

影响：

- 断电会留下半迁移非空目录，用户下次无法继续。
- 一个损坏日志标记或杀毒软件文件锁即可让应用完全打不开。

修复要求：

- staging+journal+校验+原子rename，保留源目录直到提交。
- canonical/reparse-point边界校验。
- 配置写失败恢复内存；损坏主配置回退备份或隔离。
- 启动维护best-effort，错误进入诊断，不阻断主程序。

### P0-15 真实凭据与SSH算法存在未关闭安全风险

证据：

- 动态脱敏器忽略长度小于3的凭据。
- SQLx/Reqwest/SSH底层错误先以Debug写日志，再进入Command DTO脱敏。
- RustSec命中RSA时序侧信道；Windows生产树中的russh使用`rsa 0.10.0-rc.18`，当前真实测试密钥为RSA 4096。
- 平台HTTP和MySQL TLS均可/实际被硬编码为明文模式。

影响：

- 特定错误或短凭据可能进入应用日志。
- RSA私钥认证存在官方已知时序风险。
- HTTP下RSA公钥可被中间人替换，访问Token仍通过明文HTTP返回。

修复要求：

- 底层错误只记录安全结构化码，统一脱敏后再落盘。
- 禁止短凭据或按字段安全处理，不能依赖全文替换长度阈值。
- 阶段8真实环境改用Ed25519/ECDSA密钥，或更换不受该公告影响的实现；拒绝RSA私钥并给出迁移提示。
- 生产项目要求HTTPS和MySQL TLS，开发明文模式需显式风险开关。

整改结论（检查点`6f82de8`）：

- `russh 0.63.1`关闭默认RSA Feature，仅保留AWS-LC和压缩；`rsa 0.10.0-rc.18`已从依赖图/锁文件移除，撤回的`chacha20 0.10.1`精确升级为0.10.2。内置SSH密码、错误密码及HostKey变化测试改用固定Ed25519服务端HostKey并通过。
- `rsa 0.9.10`仍由平台现有RSA公钥登录协议和SQLx MySQL引入；生产平台登录只构造`RsaPublicKey`并执行公钥加密，`RsaPrivateKey`只存在测试模块。发布配置和远端连接在两层拒绝RSA/DSA私钥，因此公告适用的SSH私钥操作不可达。
- 新增通用`SafeError`，Debug/Display只输出Rust错误类型；生产源码的SQLx、SQLite、Keyring、RSA解析、Argon2、CSV、任务及启动维护错误全部改用类型摘要。设备API响应正文和CSV未知列名不再记录。
- `stage75d_contract`扫描全部生产Rust源码并禁止原始错误/响应正文/列名模式，同时锁定russh Feature、chacha20 0.10.2并拒绝RSA 0.10 RC；全体默认测试、默认及全Feature严格Clippy通过。

## 5. P1：替换原工作台前应清零

### P1-01 Application层仍直接依赖基础设施

- 37处formal/infrastructure/SQL依赖位于`application/`。
- `aio_assets.rs`直接SQL，`deployment_service.rs`和`deployment_control.rs`直接创建Repository/SSH等具体实现。
- 修复：以业务端口重构AIO资产、项目上下文和AIO执行Lifecycle；具体组合进入infrastructure/bootstrapping。

整改结论（检查点`940566c`）：项目上下文、部署控制和具体部署服务物理迁入Infrastructure；AIO资产与Release文件入口新增Application Port和纯用例，Tauri Command作为组合根注入具体服务；AIO会话/版本/操作/平台问题等DTO及Release渲染DTO迁入Domain。Application目录对formal/infrastructure/sqlx直接依赖从45处降为0，`stage75d_contract`扫描全部Application源码阻止回退。57项库单测、全部集成测试及全Feature严格Clippy通过。

### P1-02 没有真正的全局节点并发上限

- TaskQueue worker 4只限制任务数；每任务concurrency最大5，理论可同时20个SSH/SFTP目标。
- 技术方案声明同时限制全局和每任务并发。
- 修复：注入全局RemoteOperationSemaphore，并按SSH/SFTP/CPU/DB资源分别预算。

### P1-03 进度、命令输出和日志无硬上限

- TaskProgress使用unbounded channel。
- SSH命令把全部stdout/stderr累积到Vec，同时再次写事件/JSONL。
- Task日志无单任务大小/事件数上限，应用日志无保留清理。
- 修复：有界背压、输出截断/落盘流、每任务日志配额、全局磁盘水位和显式TRUNCATED事件。

### P1-04 SFTP发布不完全原子且部署上传不校验SHA-256

- overwrite先删除旧远端文件再rename；rename失败会同时丢旧文件。
- 部署`UploadRequest.expected_sha256=None`，只校验大小。
- 修复：远端同目录backup/replace或posix rename扩展；所有Agent/Release/镜像/配置传入SHA-256。

### P1-05 成功任务可容忍远端staging清理失败

- cleanup失败只记录`REMOTE_CLEANUP_FAILED`警告，原执行仍返回成功。
- staging包含`.env`和host-info，可能长期残留。
- 修复：共享结果增加cleanup状态；敏感清理失败不能显示完全成功，提供安全重试/远端GC。

### P1-06 Release格式校验不完整

- schemaVersion只要求大于0，不拒绝未知版本。
- runtime.os/arch/docker/compose字段未校验、未参与预检。
- fingerprint遍历使用`filter_map(Result::ok)`静默忽略目录错误。
- 镜像tar条目数、Release文件数和总尺寸无上限。

整改结论（检查点`33708a1`）：schemaVersion严格等于1；runtime只接受linux/x86_64和`>=主.次[.补丁]`数字约束，要求随发布物检查结果进入每台节点预检，真实执行uname、Docker及Compose版本比较。Release目录10000文件/50GiB限制保留且归档阶段再次校验；镜像tar增加20GiB文件、100000条目、20GiB声明总量、唯一1MiB manifest限制。版本解析、非法runtime以及tar文件/条目/声明量测试通过。

### P1-07 模板值可通过换行注入额外env

- 非JSON占位符直接插入原值；`validate_env`会把注入的新行当作合法变量。
- 修复：定义明确.env编码/禁止CRLF，凭据与业务字段分类型渲染；若实际渲染Compose则使用YAML模型而非裸字符串替换。

### P1-08 远端备份与临时资产没有保留治理

- 备份在线复制SQLite类文件，可能不是一致快照。
- 不同路径同名数据库复制到同一目录可能覆盖。
- `backup/`、`service-upgrades/`和崩溃staging没有过期清理。

整改结论：已关闭。Agent 0.1.3对`$DATA_ROOT/backup`、`$DEPLOY_ROOT/service-upgrades`、`$DATA_ROOT/.inxaiot-desk-buddy`三个固定根目录的直接子目录分别执行30/14/3天超期递归清理，拒绝空/根/相对/穿越路径和符号链接根。SQLite类数据只在当前Compose停止后复制，目标保留`DATA_ROOT`相对路径；停止或复制失败仍尝试恢复，启动后验证四个既有容器。两节点真实门禁用31/29、15/13、4/2天唯一样本证明删除/保留边界，完成停启、数据库备份和运行态验证后精确删除全部样本与备份。

### P1-09 固定工作台Schema与最小数据库权限未由后端强制

- 后端只校验数据库名字符，不强制`workbench_db=inxaiot_desk_buddy`或与business_db不同。
- 同一账号同时建立平台和工作台Pool；平台只读依赖session设置和代码纪律。
- 修复：固定/受控Schema策略、明确不等约束、生产数据库最小授权门禁，最好分离只读/读写账号。

整改结论（检查点`b5d8690`）：ProjectInput强制工作台Schema使用`inxaiot_desk_buddy*`且不等于业务库；DualMySqlPools为平台连接逐连接执行`SET SESSION TRANSACTION READ ONLY`，工作台连接保持读写。授权开发MySQL只读查询兼容`transaction_read_only/tx_read_only`，实证平台标志1、工作台0，未执行平台写入。生产环境是否使用独立只读账号及实际GRANT仍属于部署门禁，不能由代码替代。

### P1-10 OS SecretStore与SQLite补偿不完整

- 项目更新事务或commit失败时可能遗留新凭据。
- session使用固定token ref，先覆盖OS Token；SQLite写失败又删除ref，会破坏原有效会话。
- 删除和旧凭据清理均忽略SecretStore删除失败。
- 修复：版本化ref、commit后切换、失败补偿表和孤儿凭据GC。

整改结论（检查点`1668896`）：数据库密码与平台Token改为UUID版本化引用；创建、项目更新和会话upsert发生错误时读取SQLite当前引用判定是否已提交，仅清理未引用新值。旧密码/Token、项目删除以及主密钥轮换/回滚删除失败统一写入`local_secret_cleanup`，启动幂等重试；`local_project_master_key`登记所有生产主密钥版本，项目删除前读取并精确清理。About显示待清理数量。SQLite Trigger与可失败SecretStore测试证明旧Token/密码保留、新值无孤儿、Outbox从1归零。

### P1-11 导入预览缺少数据库唯一打开约束

- “一个项目一个preview”依靠先查再插，双请求可同时创建。
- 修复：SQLite partial unique index或项目级CAS；增加双请求测试。

### P1-12 数据映射存在静默默认值

- 多个MySQL行映射使用`unwrap_or_default`，损坏/类型不兼容时显示空字符串或0而不是失败。
- 修复：稳定身份、状态、版本字段严格解析；仅真正可选展示字段允许默认。

整改结论（检查点`78bea8b`）：平台Schema字段、节点ID/MAC/名称/IP/状态/时间，工作台服务版本与最近操作，SQLite导入会话/选择状态，本机检查和操作历史总数全部改为严格Result映射；可空位置、备注、关联ID仍使用Option但类型错误不再被`.ok()`吞掉。数据库TLS/字符集/表/迁移探测失败不再伪装成未加密或空Schema。生产源码`try_get+unwrap_or_default/.ok`关键模式归零；真实平台只读与隔离工作台写入/严格读取/精确清理门禁通过。

### P1-13 关于页和状态栏不是实际诊断

- 本地/工作台Schema版本硬编码为`2`；无项目时仍显示工作台v2。
- 数据目录加载失败时状态栏默认显示“已生效”；诊断Promise失败未展示且可能产生unhandled rejection。
- 修复：查询实际Migration；未连接显示Unavailable；全局诊断错误态可恢复。

整改结论（检查点`837dd79`）：本地Schema版本读取真实迁移表，工作台显示迁移器实际支持版本；数据目录状态栏区分加载中、未加载、读取失败、维护异常、待重启和已生效，不再对`undefined`默认成功。About诊断失败显示真实错误与重新读取按钮，Promise拒绝被捕获。App级同时注入数据目录/诊断失败的用户测试通过。

### P1-14 会话过期和项目列表错误缺少故障隔离

- 前端`checkSession`没有任何调用点。
- 单个项目的SecretStore会话损坏可使`listProjects`整体失败。
- 初始化失败仍把Store标记initialized，缺少重试入口。

整改结论（检查点`233b9cf`）：Shell已经存在的60秒`checkProjectSession`继续保留；本地过期值缺失、非法、过去统一按Expired处理。`list_projects`逐项目捕获Overview/SecretStore异常，返回该项目隔离失败状态并继续其他项目。前端只有列表成功后才置initialized，失败保持可重试并在ProjectSwitcher显示错误/重新读取按钮；首次失败、第二次成功测试通过。

### P1-15 300节点产品能力只在后端列表存在

- 部署目标选择只使用当前AIO分页，最多20/50/100台；没有跨页选择、搜索或选择集摘要。
- “选择全部”只选择当前页。
- 镜像版本聚合也只聚合当前页，却显示为项目级摘要。
- 操作历史UI固定读取前20条，没有分页入口。

整改结论（检查点`84a105b`）：AIO Store增加独立完整选择集，按100条分页读取、MAC去重、后端total一致性校验和项目代次隔离，最大10000台；Operations的搜索、全部匹配、匹配在线和节点名称均使用完整集，选择不再受当前列表页限制。镜像版本分布打开时读取完整项目集。共享操作历史按后端page/pageSize/total显示分页控件。250节点跨3页与历史第2页测试通过。

### P1-16 服务“正常”是历史版本相等，不是实时健康

- 只要expectedVersion等于observedVersion就标healthy；observed值在成功最终化时直接写成expected。
- 列表没有服务检查时间，违反产品文档“状态必须带最近检查时间”。
- 修复：命名为“版本一致”或接入真实最近健康结果与时间，超期自动降级为未知。

### P1-17 前端系统能力没有统一Adapter

- AioNodeList、Operations和Preferences页面直接导入Tauri dialog插件，再用运行时分支模拟Fixture。
- OperationsAdapter正式接口仍包含`DemoTask`、`startFixtureTask`和旧`execute`。
- 修复：FilePickerPort/Adapter；Demo类型完全移入dev-fixtures，生产接口不出现Fixture方法。

### P1-18 跨项目异步Store没有响应隔离

- Release/AIO/Project/History Store均无AbortController或请求代次。
- 快速切换时旧请求可覆盖新项目页面；Release Store尤其可能跨项目展示完整凭据。
- 修复：每次请求携带projectId+generation，写状态前核对；切换立即清空敏感视图。

### P1-19 页面恢复与轮询策略不足

- 部署运行页每500ms读取Task、任务列表和最多500条日志，事件机制仍被重复轮询。
- 切走再返回无法恢复本实例活动任务到部署页。
- finalizing_failed不进入结果页且没有操作入口，会无限轮询。

整改结论（检查点`3405c04`）：ActivityTask DTO增加domainType/operationType，Activity Store暴露最新Task事件；部署页只在当前任务事件到达后80ms合并加载详情，任务/日志列表由Activity 120ms合并刷新，原500ms循环降为5秒事件丢失兜底。页面重入从Activity最近任务中按`aio + first_deploy/full_upgrade/service_upgrade`恢复queued/running/cancelling/finalizing_failed状态，finalizing_failed直接进入结果/保留制品说明且不无限轮询。真实App挂载恢复运行任务测试通过。

### P1-20 没有可重复的自动发布门禁

- 无CI配置。
- 35个真实测试默认被ignore，正常`cargo test`不会执行。
- Tauri bundle关闭，没有Windows安装包、签名、升级/回滚产物。
- 修复：建立分层CI、受控真实环境流水线、Release SBOM/哈希/签名和安装包。

## 6. P2：工程质量与体验改进

| 编号 | 问题 | 建议 |
| --- | --- | --- |
| P2-01 | `stage75_adapter.rs`、`aio_assets.rs`、`deployment_service.rs`和三个Vue页面体积过大 | 按用例、查询、命令、展示组件拆分，并维持端口边界 |
| P2-02 | `formal/platform_auth.rs`与`infrastructure/platform_auth.rs`重复，后者未使用 | 删除重复实现，保留一个端口适配器 |
| P2-03 | localStorage偏好未做运行时Schema校验 | 对theme/pageSize等做版本迁移和合法值回退 |
| P2-04 | 新建/登录/Schema升级存在“远端已成功、后续打开失败却整体提示失败” | 返回分阶段结果，避免用户重复提交 |
| P2-05 | 失败/取消结果仍使用CheckCircle图标，时间格式不统一 | 按真实状态使用图标并统一本地化时间 |
| P2-06 | Router没有404/恢复路由 | 增加安全重定向和错误页面 |
| P2-07 | 日志newest分页每次先全文件计数再全文件扫描 | 建立稀疏索引或反向读取，避免长任务日志O(n)轮询 |
| P2-08 | 真实桌面E2E主要依赖文字匹配，缺少可访问性、键盘、缩放和高DPI检查 | 阶段8补充Windows 10/11、缩放、键盘和屏幕阅读器基本门禁 |

## 7. 产品功能追踪结论

| 功能 | 当前达到层级 | 深审结论 |
| --- | --- | --- |
| 项目CRUD/双库测试 | 集成完成 | 活动任务守卫、固定Schema、SecretStore补偿未完成 |
| 平台登录/会话 | 主路径可用 | Token未用于持续校验，缺失过期可永久Active |
| 发布参数/凭据 | 版本冲突可用 | 密钥绑定DB密码，Compose编辑不进入部署 |
| HostKey | 主路径正确 | 私钥算法与RustSec风险需收口 |
| CSV导入/资产 | 事务和重新对账较完整 | preview唯一竞态、静默字段默认和平台问题展示缺口 |
| 一体机列表/详情 | 真实DTO可用 | 服务健康定义失真，跨项目响应和分页规模问题 |
| Release/镜像校验 | 基础安全较好 | 版本路径、schema/runtime、指纹传播和资源上限缺口 |
| 三类部署 | 两节点主路径通过 | 任务快照、回滚、取消、全局并发和最终化不满足生产不变量 |
| Task/日志/取消 | 基础骨架可用 | panic/dispatch边界、输出配额、跨项目跟踪和finalizing入口缺口 |
| 多实例/租约/恢复 | 不同实例测试通过 | 同实例接管、fencing TOCTOU、本机双进程恢复冲突 |
| 数据目录 | 正常迁移/回滚通过 | 崩溃一致性、真实路径、配置损坏和启动维护容错不足 |
| 关于/诊断 | 页面已接通 | Schema与数据目录状态可能虚假 |
| Windows发布 | 裸exe可构建 | 无独立Git、CI、安装包、签名和升级回滚 |

## 8. 现有测试没有覆盖的关键场景

以下场景在测试名称和源码匹配中均为0或不足以证明：

1. 同一数据目录启动两个真实进程。
2. 同一instanceId的两个重叠Operation抢同一MAC。
3. 预检后修改ReleaseProfile、目标IP/版本、HostKey或本地制品。
4. 旧`execute_deployment`从正式IPC绕过队列。
5. 活动任务期间编辑/删除项目。
6. 最终化每个SQL边界的故障注入、幂等重试和旧fencing拒写。
7. Handler panic/MissingHandler/dispatch取消竞态的SQLite最终状态。
8. Release版本路径穿越和Agent双重边界校验。
9. Agent compose失败、health失败、取消、断电后的自动回滚。
10. Compose模板修改后远端实际文件变化。
11. 平台Token撤销、缺失expires_in和前端实时过期。
12. 数据库密码轮换和不同账号读取同一发布凭据。
13. 数据目录复制中断、配置损坏、retention损坏和文件锁。
14. 短凭据、底层错误和超大远端输出的泄漏/资源上限。
15. 跨项目请求乱序与运行任务切项目后的持续跟踪。
16. 跨页选择300台目标和超过20条历史记录。

## 9. 修复依赖顺序

不要按页面或阶段字母零散修补，按根因依赖一次推进：

1. **建立版本基线**：独立Git仓库、冻结当前Release和本报告。
2. **封闭执行入口**：单实例锁、删除旧执行IPC、项目活动任务守卫。
3. **冻结任务事实**：不可变payload、制品指纹、Profile/Node/HostKey版本、跨项目Store隔离。
4. **重做执行一致性边界**：同实例租约冲突、全局并发、fencing单事务最终化、OutcomeReconciler和最终化重试。
5. **重做远端故障原子性**：Agent operationId、路径双校验、prepare/commit/rollback、取消与远端对账、敏感staging清理。
6. **统一Release真相**：Compose权威来源、指纹贯穿、manifest/runtime约束、env安全编码。
7. **统一认证和密钥**：持续会话验证、稳定项目主密钥、SecretStore补偿、TLS、非RSA SSH密钥、日志脱敏。
8. **加固数据目录和日志**：迁移journal、best-effort启动维护、磁盘配额和日志保留。
9. **完成用户规模与恢复**：300节点跨页选择、历史分页、活动任务恢复、真实服务状态与诊断。
10. **建立发布门禁**：CI、真实环境流水线、installer、签名、SBOM、升级和回滚。

## 10. 阶段8准入条件

只有同时满足以下条件，才允许恢复“阶段7.5完成”并进入阶段8：

- P0-01～P0-15全部关闭，且每项有自动化和对应用户/故障注入证据。
- P1中的安全、数据一致性、真实状态、300节点规模和发布门禁项关闭。
- 正式Tauri Command只有一条部署执行入口。
- 同一数据目录双进程、同实例双任务、预检后变更、项目删除、最终化故障、Agent回滚和数据目录崩溃门禁通过。
- 使用非RSA测试/验收私钥，平台和MySQL生产连接策略明确。
- 前端跨项目响应不会串数据，运行任务切项目后仍可跟踪。
- 真实Compose、Release指纹、远端版本/健康和共享历史互相一致。
- 项目进入独立Git仓库，CI与签名安装包可以追溯到同一提交。

## 11. 最终Review结论

- 当前版本适合作为已经完成大量技术验证的开发基线，不适合直接进入阶段8全量用户验收或替换原工作台。
- 本报告不是新阶段，也不形成7.5-E；所有问题统一归入“7.5-D全仓Review整改”。
- 修复完成前冻结原Go/Wails工作台退役、冻结当前Release对外分发、冻结阶段8准入。
- 后续整改只以本报告优先级和依赖顺序为准；新增问题必须说明为何现有全仓Review无法覆盖，避免继续无边界拆批次。

## 12. 2026-08-31整改执行状态

### 12.1 版本与检查点

- 工程已建立独立Git仓库；整改前基线为fa3125dbab180d2b8f5c299d009170154ce161fe。
- 第一批执行入口检查点为d437844：数据目录双进程锁、旧部署IPC删除、项目活动任务守卫。
- 第二批主整改检查点为32794e0：不可变快照、原子最终化、Outcome Reconciler、远端安全、数据目录崩溃恢复和前端项目隔离。
- 真实平台Token契约修正检查点为dfe6580。
- 当前分支为codex/stage75d-deep-review-fixes；关于页构建诊断已显示提交ID和dirty状态。

### 12.2 P0逐项状态

| 编号 | 当前状态 | 已取得证据 | 未关闭门禁 |
| --- | --- | --- | --- |
| P0-01 | 能力已关闭 | 独立仓库、系列检查点、LF规则、凭据/镜像忽略、构建内提交ID、本地门禁、内网Jenkins分层流水线及NSIS/SBOM/签名脚本 | 当前无受信Code Signing证书，首个签名安装包仍属发布阻断 |
| P0-02 | 能力已关闭，桌面门禁待补 | 锁在日志/SQLite/恢复前获取；同进程与真实子进程竞争、释放后恢复测试通过；Windows启动错误对话框已接通 | 需用正式Tauri程序执行同数据目录双启动用户门禁 |
| P0-03 | 已关闭 | execute_deployment Command、前端API、Real Adapter方法和应用层同步入口全部删除；契约测试确认仅保留preflight→submit→Handler路径 | 无 |
| P0-04 | 能力已关闭，真实门禁待补 | payload包含版本化执行快照、Profile版本、节点版本、HostKey、发布物指纹；payload SHA-256校验；发布物复制到任务专属目录后复算指纹 | 需以非RSA密钥重跑预检后修改源文件/Profile/节点/HostKey的真实阻断门禁 |
| P0-05 | 能力已关闭，用户门禁待补 | 项目活动任务前置守卫+SQLite触发器；Operations/Project/Release/AIO Store请求代次；任务始终按原项目轮询/取消；删除当前项目真实切换下一项目 | 需桌面快速切换与活动任务删除/编辑用户门禁 |
| P0-06 | 已关闭 | 唯一`poc:lease:UUID`实证同instanceId第二Operation和不同实例均冲突；释放后fencing递增，计数行精确清理为0 | 无 |
| P0-07 | 已关闭 | fencing行锁、目标结果、服务版本、资产、操作和租约释放进入同一MySQL事务；真实随机Schema证明stale fencing拒绝、中途SQL失败全回滚、成功收敛和重复拒写；执行前后information_schema残留均为0 | 无 |
| P0-08 | 能力已关闭 | 正式Outcome Reconciler已接入；缺失Handler、spawn失败、panic、abort、cancel统一持久化；dispatching状态消除取消边界；广播滞后审计未跟踪任务 | 需桌面任务panic/缺失Handler故障注入门禁 |
| P0-09 | 已关闭 | 两节点`../escape-*`以40拒绝、同版本以37拒绝，事件流均未进入Compose；current不变且fixture零残留 | 无 |
| P0-10 | 代码已关闭，Linux门禁待补 | 渲染Compose写入每节点任务文件、SFTP上传并传递REMOTE_COMPOSE，Agent覆盖Release内Compose后才启动；全部上传文件带SHA-256 | 需两节点修改Compose值后远端实际文件/容器配置变化门禁 |
| P0-11 | 故障分支与防复发代码通过，现网迁移阻断 | Agent 0.1.3两节点Compose启动失败/健康失败分别以84/86返回并恢复.env/原镜像/running；SSH取消通过；整包/rule-engine单服升级新增sqlite3 -readonly缺列阻断，首次部署/其他服务不误拦截 | node121旧rule-engine SQLite仍缺少record_type；完成受控备份迁移、服务恢复和两节点Compose配置传播后才能关闭 |
| P0-12 | 真实集成已通过，桌面过期门禁待补 | 缺失expires_in强制30分钟；每60秒真实Token只读校验；401/403清理会话，网络异常fail-closed；授权平台真实登录+ /sys/menu/nav 契约通过 | 需桌面Token撤销/到期后页面立即关闭业务入口门禁 |
| P0-13 | 能力、隔离集成和自动用户门禁已关闭 | 随机版本化项目主密钥、旧密文事务迁移、数据库密码解耦、轮换失败回滚、Windows Credential Manager、项目绑定口令包、缺钥页面恢复入口和精确清理均通过 | 需正式Tauri原生保存/打开对话框执行一次人工用户门禁；不再属于架构或数据可恢复性缺口 |
| P0-14 | 能力关闭，正常桌面门禁通过 | 既有崩溃一致性能力不变；正式Tauri空白identifier完成切换、重启生效、实际路径、回滚、再次重启和非空阻断，目录零残留 | 仍需复制中断/改名后崩溃/损坏sidecar桌面门禁 |
| P0-15 | SSH主链与故障分支通过，MySQL TLS阻断 | Ed25519两节点认证、HostKey、SFTP、Agent、P1-08、P0-09和P0-11均通过并零密钥残留；RSA拒绝、日志脱敏和依赖契约不变 | 私网Preferred TLS对开发MySQL以Rustls/Native TLS均握手失败；不得用未授权明文降级代替 |

### 12.3 已随P0关闭的P1问题

- P1-01：具体服务物理归位Infrastructure，AIO/Release建立Application Port，Domain DTO独立，Application具体依赖归零。
- P1-02全局节点并发：进程级远端节点Semaphore固定上限5，不再按4个Task worker各自放大。
- P1-03资源上限：每条远端命令输出1MiB、单条任务日志64KiB、单任务日志32MiB。
- P1-04/P1-05：所有SFTP部署上传启用SHA-256；覆盖采用可恢复备份切换；敏感staging清理失败不能返回成功。
- P1-06：schema/runtime语义、Release归档与镜像tar资源上限完成；runtime进入逐节点OS/架构/Docker/Compose真实预检。
- P1-07：非JSON模板变量含CR/LF/NUL立即拒绝。
- P1-09：工作台Schema边界和平台session只读完成，开发真实标志通过；生产GRANT待部署门禁。
- P1-10：密码/Token版本化引用、主密钥本地登记、SecretStore清理Outbox、启动重试和About待清理诊断通过故障注入。
- P1-11：SQLite部分唯一索引确保每项目只有一个开放导入预览，并发测试通过。
- P1-12：平台/工作台/SQLite事实字段严格映射，真实平台只读与隔离写入/清理验证通过。
- P1-13/P1-16/P1-17/P1-18：Schema/数据目录/诊断真实状态，服务15分钟观测，系统对话框Adapter和四类Store项目代次隔离完成。
- P1-14：60秒Token验证、非法过期fail-closed、单项目列表故障隔离和初始化可见重试完成。
- P1-15：完整节点选择集、全项目搜索/全选、项目级版本聚合和共享历史分页完成，250节点跨页测试通过。
- P1-19：Task事件合并刷新、5秒安全兜底和页面重入活动/finalizing_failed任务恢复完成。
- P1-08：Agent 0.1.3停服一致性备份、服务相对路径、失败恢复、四容器验证及三个固定根30/14/3天治理通过两节点门禁。
- P1-20：内网Jenkins、NSIS、CycloneDX、哈希、Authenticode、CMS清单签名、不可覆盖产物集与离线验证代码已建立；因当前无企业CA/既有证书且自签名Root/TrustedPublisher信任变更未获精确授权，仍不视为关闭。

### 12.4 本轮门禁证据

- 前端：typecheck、严格Lint、18个测试文件39项测试、生产构建通过；新增主密钥/缺钥恢复、项目重试、250节点跨页、历史分页、Task事件、Operations重入和诊断错误态契约。
- Rust：cargo fmt --check、全目标全Feature严格Clippy通过；默认Feature全部非忽略单元/集成测试通过（库单测57项）；新增SSH依赖/原始日志契约、SecretStore补偿、Release runtime/tar上限和非法会话过期测试通过。
- 故障注入：真实双进程锁、本地最终化中途失败全回滚/重试、并发导入唯一约束、payload篡改、旧项目响应晚到、会话校验不可用fail-closed均通过。
- 真实平台：只执行授权测试登录和Token只读菜单校验，未连接、未执行SQL、未修改平台业务库结构或数据。
- 主密钥真实集成：仅在授权隔离工作台Schema写入`key-poc-*`唯一配置/审计，覆盖迁移、数据库密码解耦、轮换失败回滚、成功轮换和跨电脑导入，结束后按唯一键删除并复查总数为0；Windows Credential Manager唯一测试引用已删除。
- 最终默认Feature生产Release基于`a0008b62fd09af4530ee08c2e9b357652f091bdb`，大小11987968字节，SHA-256为`FBE660633418908A1784A002899C042DFE893EBCCB34B8E347C7BF5DBB65A1D3`；包含三项主密钥Command，无WebDriver/Fixture标记，已覆盖1668896至b5d8690的全部P1代码整改。依赖树不存在`rsa 0.10.0-rc.18`，`chacha20`为0.10.2。
- 真实最终化：随机隔离Schema门禁通过stale fencing、事务触发器失败、成功/重复最终化；门禁前后`FINALIZATION_SCHEMA_RESIDUE_COUNT=0`。
- 真实SSH/Docker：两节点临时Ed25519通过HostKey、2MiB SFTP、Agent和P1-08 Compose停启/恢复；唯一远端测试资产、公钥和本机密钥目录均精确删除。现有RSA 4096测试私钥仍保持拒绝。
- 发布基础设施：CycloneDX 1.5实测905组件，Tauri发布配置完成debug/no-bundle构建；未签名NSIS冒烟包成功生成并被断言为`NotSigned`，记录大小/哈希后删除，临时配置零残留。发布脚本以target内临时配置注入签名脚本绝对路径，正式产物根要求管理员预置并拒绝宽泛写ACL。创建自签名证书及加入CurrentUser Root/TrustedPublisher的命令在执行前被安全审查拒绝；复查My/Root/TrustedPublisher计数均0，`D:\inxaiot-release-artifacts`不存在，无持久副作用。
- 真实租约/Agent：P0-06唯一计数行清理为0；P0-09两节点在Compose前拒绝；P0-11两节点Compose/健康故障回滚和SSH/SFTP取消通过，第二批临时Ed25519及`p009/p011`资产零残留。
- 正式Tauri：7.5-D数据目录切换、重启、About实际路径、回滚和非空阻断通过，专用数据目录清理完成；Computer Use辅助进程连续初始化失败后停止，未用PowerShell UI Automation绕过。
- CI稳定性：完整App挂载在全量并发下实际耗时超过15秒，测试超时容量校准为30秒但断言完全不变；复跑18文件39项全部通过，避免发布流水线偶发假红。
- 新阻断证据：Stage75Adapter的Preferred TLS在开发MySQL返回`platform-connect:io(kind=InvalidData)`，Native TLS同样失败且已回退，所有stage75b随机Schema残留为0；node121 rule-engine日志与只读PRAGMA证明`rule_definition`缺少`record_type`，node79同表具备该列，未修改任一边缘数据库。
- rule-engine防复发：现有SSH预检新增只读Schema检查，缺列时阻断性remediation为`migrate_rule_engine_schema`；四项测试证明检查只在会重启rule-engine时运行、命令带`-readonly`、缺列阻断、存在通过、缺数据库警告且生产源码无ALTER。

### 12.5 当前准入结论

- 阶段7.5-D仍为“整改中”，阶段8继续暂停。
- P0-06/P0-07/P0-09与P1-08已关闭，Ed25519双节点主链及P0-11故障回滚分支已通过；代码门禁仍不能替代剩余正式桌面和跨服务数据迁移门禁。
- 当前三个外部/环境阻断为：node121 rule-engine SQLite受控备份迁移与恢复、开发MySQL受信TLS配置（或用户精确接受测试明文风险）、P1-20受信签名证书与首个安装包。任何一项未关闭都不能进入阶段8。
- 阶段7.5-D仍为“整改中”，阶段8继续暂停；不得以重启掩盖Schema漂移，也不得把未受信安装包记为完成。
