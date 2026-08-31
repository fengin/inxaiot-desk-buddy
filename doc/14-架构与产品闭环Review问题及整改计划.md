# 架构与产品闭环 Review 问题及整改计划

| 属性 | 内容 |
| --- | --- |
| 文档日期 | 2026-08-31 |
| 适用项目 | `inxaiot-desk-buddy` |
| 输入基线 | 产品设计v0.4、界面规范、技术方案v1.15、阶段0～7.5代码与验收记录 |
| 当前状态 | P0-13稳定项目主密钥能力、隔离MySQL集成和自动用户门禁已关闭；非RSA双节点、正式桌面故障场景及外部发布门禁仍阻断阶段8 |
| 目标 | 修复 P0～P2 问题，形成可从空白安装开始操作的真实桌面产品，并为智能屏、网关保留稳定扩展内核 |

## 1. Review 结论与阶段状态纠正

代码级 Review 确认，现有 Rust 后端已经验证双数据库、工作台数据、SSH/SFTP、Agent、租约/fencing、恢复和三类部署等核心能力，但此前把“组件或服务层门禁通过”过早等同于“用户功能闭环完成”。

阶段7应拆分为两个事实：

- 阶段7-A（已完成）：后端多实例、恢复、性能、双节点三类部署和 Release 技术门禁。
- 阶段7.5（整改重开）：7.5-A～D原三层门禁均已执行，但全仓深度Review确认这些门禁没有覆盖进程独占、不可变任务快照、租约/fencing最终化、Agent回滚、会话真实性等关键不变量；问题统一归入7.5-D收口，不新增7.5-E。
- 阶段8（暂停准入）：继续只承担全量用户验收和替换决策；在深度Review的P0问题关闭前不得开始。

原三层门禁通过是历史事实，但不再构成阶段完成结论。当前唯一问题基线为`doc/15-全仓代码与产品深度Review分析报告.md`；原 Go/Wails 工作台继续保留，当前Release冻结对外分发。

## 2. 问题清单

### 2.1 P0：阻止真实用户闭环

| 编号 | 问题 | 当前证据 | 用户影响 | 目标状态 |
| --- | --- | --- | --- | --- |
| P0-01 | 项目新增、编辑、切换和登录仍操作 Demo Store | `ProjectSwitcher.vue` 未调用真实项目 Repository、平台认证和会话 Command | 全新安装无法通过界面建立真实项目 | 项目 CRUD、连接检查、平台登录、独立会话全部使用真实 Tauri API |
| P0-02 | 发布参数页面只读写 Demo 数据 | `AioReleaseProfileView.vue` 未接 `ReleaseProfileRepository` | 用户无法创建部署必需的真实发布参数 | 读取、编辑、校验、乐观锁冲突和凭据保存形成真实闭环 |
| P0-03 | 部署页混用真实执行与 Demo 目标、进度、结果和历史 | 页面从 `demo.nodes/tasks/history` 取数，检查项和结果存在固定成功文案 | 可能显示错误目标、0%进度或把部分失败显示为成功 | 目标、预检、任务、节点结果、进度、日志和历史全部来自真实 DTO |
| P0-04 | 主机密钥确认无产品入口 | 后端执行要求已确认 HostKey，但没有捕获/确认/变更确认 Command 和界面 | 首次真实部署没有可操作的解阻路径 | 检查阶段支持首次确认、变化阻断和明确重新确认 |

### 2.2 P1：架构、可靠性与扩展性

| 编号 | 问题 | 当前证据 | 扩展/运行风险 | 目标状态 |
| --- | --- | --- | --- | --- |
| P1-01 | 生产部署未进入 `TaskQueue`，`JobSupervisor` 只注册 Token、不拥有执行句柄 | Tauri Command 等待完整部署；`TaskQueue` 只在测试使用 | 安全退出无法真正等待；智能屏/网关会形成独立调度路径 | 提交任务立即返回 Task ID，生产队列统一路由并持有 JoinHandle |
| P1-02 | 上传、远端输出和 Agent 事件使用 Noop Sink | 部署未把 Transfer/Command 输出接入事件管线 | 用户看不到实时阶段、进度和诊断；活动面板价值不足 | 每个步骤产生单调事件、目标进度和脱敏 JSONL 日志 |
| P1-03 | 通用执行控制与一体机最终资产逻辑混合 | `deployment_control.rs` 同时处理任务、租约、操作和 AIO 版本资产 | 新业务容易复制控制面代码 | 抽取通用 `ExecutionCoordinator`，业务 Handler 只提供计划和最终资产策略 |
| P1-04 | 数据目录设置只保存到前端 localStorage | Rust 启动仍固定使用 Tauri `app_data_dir` | 用户选择目录后实际数据位置不变 | 数据目录由启动配置读取；变更需校验、迁移/切换并重启生效 |
| P1-05 | 部署任务目录缺少临时文件清理 | Release副本、Agent、渲染 `.env` 和 host-info 保留 | 长期磁盘膨胀、敏感配置残留 | 日志与临时制品分目录；按成功/失败/保留策略精确清理 |
| P1-06 | Agent 通过相对路径引用兄弟工程 | `include_str!` 依赖 `inxaiot-edge-workbench` | 单仓构建失败；后续多 Agent 版本不可控 | Agent 作为当前工程版本化资源或显式共享构建依赖 |
| P1-07 | Application 直接实例化 SQLx/SSH/业务 Repository | 应用层依赖具体 infrastructure/formal 实现并存在直接 SQL | 测试替换困难，屏/网关编排耦合增长 | AppState/项目运行时注入端口；应用用例只依赖领域与端口 |
| P1-08 | 真实项目会话守卫只覆盖部分写操作 | 列表等读取路径只检查数据库，不统一检查项目登录状态 | 未登录/过期时仍可能进入业务页面 | 增加统一 `ProjectAccessGuard` 和前端路由/菜单状态守卫 |

### 2.3 P2：体验、测试和文档一致性

| 编号 | 问题 | 影响 | 目标状态 |
| --- | --- | --- | --- |
| P2-01 | 状态栏固定显示数据库已连接，项目和用户仍来自 Demo | 连接失败时给出错误反馈 | 状态栏使用真实项目运行时、Schema和会话状态 |
| P2-02 | 关于页仍显示阶段4，多个按钮无处理逻辑 | 用户误判版本和可用能力 | 所有可见按钮必须可用、禁用并解释，或删除 |
| P2-03 | 前端测试全部走浏览器 Fixture | Demo/真实后端断层无法被发现 | 增加 Store→Tauri Command契约测试和桌面端E2E |
| P2-04 | 阶段记录把后端门禁表述成产品完成 | 计划与实际状态失真 | 验收记录区分能力、集成和用户三层门禁 |
| P2-05 | 运行时脱敏器未注册项目真实凭据 | 文本日志仍有泄漏可能 | 项目上下文动态注册敏感值，Command错误也经过统一脱敏 |

### 2.4 2026-08-30第一轮整改历史状态

下表编号和完成状态仅对应第一轮Review问题，不等同于`doc/15`重新编号的P0/P1问题。深度Review发现的阻断项以`doc/15`为准，不能用下表“已完成”抵消。

| 编号 | 当前状态 | 证据与剩余门禁 |
| --- | --- | --- |
| P0-01 | 7.5-A已完成 | Project Store→Real Adapter→Command→应用端口→真实适配器纵向链路通过；空白数据目录真实Tauri界面完成项目新增、双库测试、切换、登录及界面删除清理 |
| P0-02 | 7.5-A已完成 | 发布参数读取、校验、加密凭据、版本1/2界面保存、旧版本冲突真实门禁和审计证据通过 |
| P0-04 | 7.5-A已完成 | 两台授权节点通过真实界面完成捕获和首次确认；变化阻断与重新确认由Repository、真实连接和界面入口共同覆盖 |
| P0-03 | 7.5-B已完成 | 真实AIO目标、逐项预检、立即Task ID、进度/日志、严格结果、共享历史和详情携参形成纵向闭环；两节点真实集成与Tauri页面成功/取消门禁通过 |
| P1-01 | 7.5-C已完成 | TaskQueue/HandlerRegistry进入正式AppState，AIO提交通过安全payload入队；JobSupervisor持有真实JoinHandle，关闭等待和强制中断不丢失句柄 |
| P1-03 | 7.5-C已完成 | ExecutionCoordinator通过生命周期端口统一启动、心跳、执行、中断和最终化；AIO Handler只保留MAC、Release、Agent、注册和版本资产策略 |
| P1-02 | 7.5-B已完成 | SFTP字节进度、SSH输出、Agent事件、目标步骤和动态凭据脱敏JSONL进入同一事件管线，真实日志必需事件缺失0、敏感值泄漏0 |
| P1-04 | 7.5-D已完成 | Rust在打开SQLite/日志前解析启动选择器；迁移、空目录切换、失败保持原目录和上一目录回滚均有能力测试与真实Tauri重启门禁 |
| P1-05 | 7.5-D已完成 | `task-logs`与`task-artifacts`物理分离；终态立即删除Release/Agent/payload/明文渲染制品，日志30/90天保留和超期删除可测试 |
| P1-06 | 7.5-D已完成 | Agent进入当前工程资源目录，版本0.1.0、协议1和SHA-256固定；正式源码/测试兄弟仓库引用0，独立检出Release通过 |
| P1-07 | 7.5-C/D按既定增量边界关闭 | 通用ExecutionCoordinator、DataDirectoryService和ProjectAccessGuard只依赖端口；存量AIO生命周期适配代码仍保留既有具体Repository组合，不新增SQL/基础设施创建。该文件位置技术债不阻塞7.5产品闭环；未来screen/gateway必须走端口，禁止复制 |
| P1-08 | 7.5-D已完成 | `ProjectAccessGuard`覆盖一体机读写、导入、部署和共享历史；前端未就绪时不挂载业务页面，后端会话缺失/过期继续fail-closed |
| P2-01 | 7.5-A/D已完成 | 状态栏、菜单、项目切换器和数据目录段读取真实项目、数据库、Schema、会话和实际Rust路径 |
| P2-02 | 7.5-D已完成 | 关于页显示应用/Schema/Agent/平台/实际路径；全Vue可见按钮动作/禁用/触发器扫描违规0 |
| P2-03 | 7.5-D已完成 | Real Adapter覆盖项目、部署、数据目录、诊断Command；生产Demo误接扫描、15文件26测试、三批Windows Tauri E2E、独立检出构建通过 |
| P2-04 | 7.5-D已完成 | doc/02、doc/03、doc/05、doc/14按能力/集成/用户三层记录实际证据；阶段7.5完成与阶段8/退役未完成明确分开 |
| P2-05 | 7.5-D已完成 | 数据库、平台登录、AuthKey、MQTT、SSH凭据进入进程级动态注册表；事件、JSONL、Command错误DTO和错误日志摘要统一脱敏，随机秘密契约泄漏0 |

本表保留第一轮整改当时实际达到的层级。2026-08-31全仓深度Review已经推翻“P0产品断点已清零”的阶段性结论；新的关闭状态须在`doc/15`清单逐项复验后回写。

## 3. 保留与调整的架构边界

以下设计继续保留：

- Rust + Tauri 模块化单体，不引入 Sidecar。
- 本地 SQLite 保存多项目入口、实时任务、步骤、日志索引和导入过程。
- 工作台 MySQL 保存共享配置、最终资产、操作摘要、节点最终结果和跨实例租约。
- 平台业务库保持只读；平台注册继续通过一体机既有 API 链路。
- `operation_record`、`operation_target_result`、`resource_lease` 和本地通用任务表继续跨业务复用。
- 一体机、智能屏、网关分别使用 `aio_`、`screen_`、`gw_` 业务表和独立领域模型。
- SSH、SFTP、文件、任务事件和日志保留通用端口。

需要调整的核心是“组合方式”：通用能力必须在生产业务路径中真正使用，而不是只存在实现和测试。

## 4. 阶段7.5实施计划

### 4.1 7.5-A：真实项目与共享配置入口（P0）

实施内容：

1. 增加项目列表、新增、编辑、删除、连接测试和切换 Tauri Command。
2. 增加平台登录、会话读取、过期检查和退出登录 Command。
3. 建立真实 `project` Store；浏览器 Fixture 通过独立 Adapter 提供相同 DTO。
4. 增加发布参数读取、保存、校验和版本冲突 Command/Store。
5. 状态栏、菜单和项目切换器统一读取真实项目状态。
6. 增加 HostKey 捕获、首次确认、变化阻断和重新确认入口。

完成门禁：

- 空白数据目录启动后可只通过界面创建并连接项目。
- 可使用真实平台账号登录；切换项目时会话互不污染。
- 发布参数可以新增、刷新、编辑并正确处理并发版本冲突。
- 首次 SSH 检查可由用户确认主机指纹。

当前实施结果（2026-08-30）：

- 已完成项目、会话、发布参数、HostKey的页面/Store/Real Adapter/Tauri Command/应用端口/真实适配器纵向链路。
- 浏览器Fixture分别位于`src/dev-fixtures/*Adapter.ts`，Tauri生产页面和Store不再依赖`DemoStore`；生产构建不含已知Fixture项目和凭据标识。
- 前端9个测试文件18项、生产构建、Rust严格Clippy和全量自动化69项通过；真实集成使用随机隔离Schema `inxaiot_desk_buddy_stage75int_01a04e6610bc`，平台登录、发布参数乐观锁和双节点HostKey通过，清理剩余0，平台业务表快照未变化；标准Windows Tauri Release构建通过。
- 独立验收应用标识从空白目录启动时本地项目、会话、HostKey均为0；真实Tauri WebDriver只通过界面完成项目新增、双库测试、真实登录、发布参数版本1/2保存、两台HostKey确认和项目删除，最终返回`STAGE75A_UI_GATE_PASS`。
- 真实界面门禁发现并修复项目表单响应式Proxy无法`structuredClone`的缺陷；隔离Schema删除前发布配置1条、最高版本2、审计2条、Migration 2条，删除后剩余0；本地项目/会话/HostKey清理后均为0。
- `desktop-e2e` Feature、稳定`data-testid`和完整真实界面脚本保留为回归资产；标准生产依赖图与最终Release确认不包含WebDriver或隔离Schema标识。

### 4.2 7.5-B：真实部署用户闭环（P0）

当前实施结果（2026-08-30，已完成）：

- 真实目标、统一预检、异步Task、进度/日志、节点结果、共享历史和详情携参均完成页面→Store→Real Adapter→Command→应用端口→真实适配器纵向链路。
- 预检真实执行制品、会话、数据库、Schema、发布参数、目标、租约、固定HostKey、SSH、Docker和Compose检查；固定成功文案扫描0，失败项返回阻断原因与整改入口。
- submit_deployment先持久化Task并立即返回ID，JobSupervisor持有JoinHandle；页面通过Task DTO/事件显示进度并可请求取消，取消状态迁移竞态已修复。
- SFTP、SSH、Agent、目标和步骤事件进入有序管线并动态脱敏；成功、失败、取消和中断只由真实最终结果派生，远端staging在成功/失败/取消后均精确清理。
- 随机隔离Schema真实集成门禁在两台授权节点完成服务升级、运行中取消、第二实例共享历史和平台只读快照，耗时168.11秒并清理剩余0。
- 独立Tauri应用从空白数据目录完成项目/登录/发布参数/HostKey后，通过部署页形成2成功结果和真实取消结果，脚本返回STAGE75B_UI_GATE_PASS。
- UI Schema证据：单服操作2、成功1、取消或部分成功1、终态节点4、pending 0、活动租约0、节点2、发布参数版本2、Migration 2；本地项目/会话/HostKey/Task删除后均为0，Schema和应用目录清理剩余0。
- 前端类型、严格Lint、11个测试文件20项、生产构建、Rust全目标全Feature严格Clippy和全量非忽略测试通过；标准Release不含WebDriver、隔离Schema或Fixture标识。
- 7.5-B未引入通用TaskQueue/HandlerRegistry或ExecutionCoordinator，相关架构收敛仍严格属于7.5-C。

实施内容：

1. 部署目标改为真实一体机 Store。
2. 实现统一预检用例，返回逐项状态、阻断原因和修复入口。
3. 提交部署后立即返回 Task ID，页面通过任务 DTO 和事件显示进度。
4. 接入上传、SSH输出、Agent事件、节点阶段和脱敏日志。
5. 结果页严格按 `DeploymentExecutionSummary.targets` 展示成功、失败、取消和错误摘要。
6. 操作历史和节点结果查询真实 `operation_record/operation_target_result`。
7. 一体机详情可带入当前节点进入部署页。

完成门禁：

- 不允许任何检查项用固定“通过”文案代替真实结果。
- 部分成功时数量、节点颜色、失败阶段和错误摘要准确。
- 用户可以在执行中查看日志、请求取消，并在重启后看到中断状态。
- 其他实例可以看到共享历史，但看不到本机完整日志和发布文件。

### 4.3 7.5-C：通用执行内核收敛（P1）

当前实施结果（2026-08-30，已完成）：

- TaskHandlerRegistry和TaskQueue已注册到正式AppState；生产配置为容量100、worker 4，容量限制运行中与排队任务总数。
- submit_deployment原子保存本地payload，将Task推进Queued后入队；AIO三类Handler统一注册并校验payload路径、操作类型和资源集合。
- Queue worker安全等待真实JoinHandle完成后再join；shutdown合并排队、worker和Supervisor结果，排队任务取消、运行任务等待/中断均持久化。
- ExecutionCoordinator只依赖ExecutionLifecyclePort，统一start、heartbeat、execute、stop heartbeat、finalize、interrupted和finalizing_failed；FinalizingFailed保持显式状态。
- screen和gateway测试Handler不依赖AIO即可注册同一队列；全局优先级、容量、取消和跨业务并发门禁通过。
- 窗口CloseRequested遇到活动任务会prevent_close并展示影响弹窗；取消关闭后任务继续，确认关闭时Running先进入Cancelling并按5秒安全边界收敛。
- 两节点真实测试经完整生产内核路径通过，随机隔离Schema清理剩余0；最终Tauri门禁返回STAGE75B_UI_GATE_PASS并验证关闭影响弹窗。
- 排队取消没有创建第二个共享Operation、租约或远端步骤；共享证据为执行操作1且成功、成功目标2、pending 0、活动租约0，本地QUEUE_CANCELLED事件存在。
- 强制退出故障门禁按stale协议恢复共享操作Interrupted，pending 0、活动租约0；精确远端staging和隔离数据全部清理。
- 前端11个测试文件20项、类型、严格Lint和生产构建通过；Rust lib单元38项、TaskQueue定向6项、全量测试和全目标全Feature严格Clippy通过。
- 标准Release为12000768字节，默认依赖树和产物中的WebDriver、隔离Schema及Fixture标识匹配0。

目标结构：

```text
TaskSubmissionService
  → TaskQueue
  → TaskHandlerRegistry
      ├─ aio::{first_deploy, full_upgrade, service_upgrade}
      ├─ screen::{future handlers}
      └─ gateway::{future handlers}
  → ExecutionCoordinator
      ├─ OperationRepository
      ├─ ResourceLeaseService
      ├─ JobSupervisor
      ├─ ProgressSink / LogSink
      └─ FinalizationPolicy
```

实施内容：

1. 把队列和 Handler Registry 注册到正式 AppState。
2. `JobSupervisor` 必须拥有实际任务 JoinHandle。
3. 抽取通用启动、租约心跳、取消、中断、最终化失败和释放流程。
4. 一体机 Handler 只保留 MAC、Release、Agent、本地API注册和版本资产逻辑。
5. 定义智能屏/网关 Handler 接入契约，不提前创建空业务实现。
6. 应用关闭时展示活动任务影响，等待安全边界并持久化最终本地状态。

完成门禁：

- 同一队列可以注册一个测试 `screen` 或 `gateway` Handler 而不依赖 `aio::*`。
- 全局队列容量、优先级、取消和跨业务并发限制实际生效。
- 关闭应用时监督器能等待或明确中断真实执行句柄。

### 4.4 7.5-D：数据生命周期、测试和文档（P1/P2）

实施内容：

1. 数据目录改为 Rust 启动配置；设计首次设置、切换、迁移和回滚。
2. 任务日志和临时制品分开保存；成功、失败和超期清理规则可测试。
3. Agent 资产移入当前工程并记录协议版本、哈希和兼容矩阵。
4. 项目真实凭据注册到脱敏器；错误 DTO 和日志统一脱敏。
5. 增加真实模式 IPC 契约测试、错误态测试和 Windows 桌面端 E2E。
6. 修正状态栏、关于页、禁用状态、空状态和所有无事件按钮。
7. 更新产品、技术、实施、验收和替换结论。

完成门禁：

- 数据目录切换后实际 SQLite、日志和任务目录与界面一致。
- 部署完成后只保留策略允许的日志/诊断信息，不保留无期限的Release副本和明文渲染凭据。
- 单独检出 `inxaiot-desk-buddy` 可以完成构建。
- Tauri真实模式测试可以发现 Demo Store 误接入。

实际完成证据（2026-08-31）：

- 能力：数据目录迁移/回滚/失败保持、任务终态制品删除、成功30天/失败90天日志保留与超期清理、Agent版本/协议/哈希、ProjectAccessGuard和Command错误动态脱敏测试通过；Rust库45项和全部非忽略集成测试通过。
- 集成：DataDirectory Store/Real Adapter/Command/Service/Port/Manager与诊断纵向链路接通；生产Demo/Fixture误接和无动作按钮扫描违规0；前端15文件26测试、typecheck、严格Lint、生产构建及Rust全目标全Feature严格Clippy通过。
- 用户：`STAGE75D_UI_GATE_PASS`。空白Tauri目录经设置页迁移，重启后实际SQLite、应用日志、任务日志、任务制品目录与关于页一致；界面回滚重启后恢复默认目录；非空目标明确阻断。
- 独立检出：无Git/依赖/构建产物的临时副本中兄弟仓库引用0，离线安装锁定依赖后完成前端测试/Lint/生产构建、Rust全量非忽略测试和标准Windows Release；临时副本随后精确删除。
- 最终产物：11792896字节，SHA-256 `F2CFB01C82098BEF046D46F3B324AC51336F29F291137205789B7405EAACA2ED`；默认依赖树WebDriver 0，Release/前端Fixture、Demo、E2E禁用标识0。
- 清理：默认验收目录、迁移目录、非空阻断目录、临时独立检出和运行进程均为0；7.5-D未连接或修改平台业务库。

## 5. 需求追踪矩阵

每项功能必须维护以下完整链路：

| 产品需求 | 页面/交互 | 前端Store/Adapter | Tauri Command | 应用用例 | Repository/外部适配器 | 自动化测试 | 用户验收 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 项目管理 | 项目切换器 | ProjectStore | project_* | ProjectApplicationService | LocalProjectRepository/SecretStore | IPC契约+SQLite | 空白安装创建项目 |
| 平台登录 | 登录弹窗 | ProjectSessionStore | login/logout/session | PlatformSessionService | PlatformAuthAdapter/SecretStore | 协议+过期态 | 登录、过期、重登 |
| 发布参数 | 发布参数页 | ReleaseProfileStore | release_profile_* | ReleaseProfileService | ReleaseProfileRepository | 版本冲突+加密 | 两实例刷新/冲突 |
| 主机密钥 | 预检/确认框 | HostKeyStore | host_key_* | HostKeyService | HostKeyRepository/RemoteConnector | 首次/变化 | 用户确认后继续 |
| 部署任务 | 部署四阶段 | Task/DeploymentStore | submit/cancel/query | TaskSubmission/Handler | Queue/Lease/Remote/Operation | 集成+真实远端 | 完整部署闭环 |
| 操作历史 | 历史列表/详情 | OperationHistoryStore | operation_history_* | OperationQueryService | OperationRepository | 查询/分页 | 跨实例查看结果 |
| 数据目录 | 设置 | DataDirectoryStore/RealDataDirectoryAdapter | data_directory_* | DataDirectoryService/DataDirectoryPort | DataDirectoryManager/AppPaths | 迁移/回滚/错误态 | `STAGE75D_UI_GATE_PASS`，重启后路径一致 |

新增功能若追踪矩阵存在空列，不得标记为完成。

## 6. 统一完成定义（Definition of Done）

### 6.1 能力门禁

- 领域模型、Repository、外部适配器和错误分支有自动化测试。
- 数据写入有事务、并发版本或租约保护。
- 平台业务库保持只读。

### 6.2 集成门禁

- 真实前端 Store 已调用正式 Tauri Command。
- Command 已进入应用用例，不从界面直接访问基础设施。
- 生产路径使用正式队列、监督器、事件和日志端口。
- 浏览器 Fixture 与真实 Adapter 明确隔离。

### 6.3 用户门禁

- 从全新数据目录启动，无需手工SQL或测试脚本。
- 用户只通过界面完成该功能的主路径和关键失败路径。
- 界面展示的数据与数据库、远端实际结果一致。
- 所有可见按钮都有有效动作、明确禁用原因或被移除。

只有三层门禁全部满足，阶段或功能才能标记“已完成”。

## 7. 阶段7.5最终验收场景

必须至少覆盖：

1. 空白安装后选择数据目录、创建项目并测试双数据库。
2. 登录平台，关闭并重启后恢复同项目会话；切换另一项目要求独立登录。
3. 初始化/升级空工作台库，不修改平台业务库。
4. 新建和编辑发布参数；两实例同时编辑时后保存者收到版本冲突。
5. 导入真实CSV，完成新增、平台既有、冲突和重复MAC处理。
6. 首次连接两台一体机并确认HostKey。
7. 完成首次部署、整包升级和单服务升级，查看实时步骤、进度和日志。
8. 构造一个节点失败，结果页和共享历史准确显示部分成功及失败阶段。
9. 执行中取消、数据库断连、应用退出并重启，确认不自动重放远端命令。
10. 在第二工作台实例查看共享结果并验证资源锁冲突。
11. 切换数据目录并重启，确认实际数据路径一致且迁移失败可回滚。
12. 单独检出当前工程完成前端测试、Rust测试和Windows Release构建。

## 8. 实施与状态管理

- 整改按 7.5-A → 7.5-B → 7.5-C → 7.5-D 顺序推进。
- A/B/C原批次和D原门禁均已执行；深度Review整改继续归入7.5-D，不追加7.5-E。
- P0未清零前不开始智能屏和网关具体业务功能。
- 7.5-D整改进行中；后续screen/gateway必须通过统一TaskEnvelope、HandlerRegistry和ExecutionCoordinator接入，不得复制一体机任务编排。
- 阶段8仍只做基于真实产品闭环的全量用户验收，不承担补接核心功能；当前因`doc/15`的P0问题暂停准入。
- 2026-08-31执行记录：A/B/C/D原能力/集成/用户门禁均通过；随后完成的全仓深度Review发现15组P0、20组P1和8组P2，原“阶段7.5整体完成”结论撤回。
- 修复顺序、故障注入场景和阶段8准入条件统一见`doc/15-全仓代码与产品深度Review分析报告.md`。
- 原Go/Wails工作台退役条件仍未完成；必须等待阶段8全量用户验收和替换/回退窗口决策。

## 9. 深度Review整改执行补充（2026-08-31）

- 已建立独立Git基线和三段检查点，完成执行入口、不可变快照、原子最终化、Outcome Reconciler、Compose真实落地、Agent 0.1.2回滚、数据目录崩溃恢复、会话真实校验和前端项目隔离；逐项状态及证据见doc/15第12节。
- 当前不得把“代码已关闭”替代真实门禁：现有RSA 4096测试私钥已被新策略拒绝，需授权Ed25519/ECDSA密钥后重跑两节点Linux门禁。
- P0-13已关闭代码根因：随机版本化项目主密钥进入Windows Credential Manager；旧数据库密码密文自动事务迁移；数据库密码可独立轮换；主密钥轮换具备失败回滚和审计；跨电脑仅使用绑定项目事实与版本的口令保护包，导入前以当前密文实际验证。正式Tauri原生文件对话框仍列为用户门禁，不再是加密架构缺口。
- P0-15代码根因已关闭：russh不再编译RSA私钥认证Feature，撤回chacha20已升级，内置SSH握手使用Ed25519；所有底层错误日志只保留类型，原始设备消息/CSV列名不落日志，并有源码/依赖静态契约。真实双节点仍需获授权Ed25519/ECDSA密钥后验收，不能用代码门禁替代。
- P1-10已关闭：数据库密码、Token和项目主密钥均有版本化/可枚举本机引用；SQLite失败保留旧有效值；SecretStore删除失败进入Outbox并在启动重试；About展示待清理数量，项目删除覆盖主密钥。故障注入已证明失败补偿和重试归零。
- 全仓深审P1-06已关闭：Release runtime从非空字段变为可执行契约，节点OS/架构/Docker/Compose不满足manifest即阻断；镜像tar和Release归档有文件、条目、声明总量与重复manifest上限。
- 全仓深审P1-12已关闭：数据库事实字段严格映射，合法NULL与损坏/类型不兼容明确区分；真实平台只读和隔离工作台写入/严格读取/精确清理通过。
- 全仓深审P1-14已关闭：真实Token定期校验、非法过期fail-closed、单项目会话错误隔离和项目列表初始化可见重试均有自动门禁。
- 全仓深审P1-15已关闭：完整项目节点选择集、跨页搜索/匹配全选、项目级版本分布和共享历史分页均已接通，250节点跨页测试通过。
- 全仓深审P1-01已关闭：具体部署/项目/AIO/Release服务归位Infrastructure，Application只保留Domain/Port/用例；直接formal/infrastructure/sqlx依赖归零并有静态架构门禁。
- 全仓深审P1-19已关闭：Task事件合并刷新替代500ms重复轮询，5秒保底；页面重入按类型恢复活动/finalizing_failed部署任务，App级用户测试通过。
- 全仓深审P1-13已关闭：数据目录和诊断失败不再显示虚假成功，状态栏/About错误可见、可重试且无未处理异步拒绝。
- 全仓深审P1-09代码/开发集成已关闭：固定工作台Schema边界、平台session只读和工作台读写真实标志通过；生产账号GRANT最小化仍待部署验收。
- 最终默认Feature Release基于a0008b6，大小11987968字节，SHA-256为FBE660633418908A1784A002899C042DFE893EBCCB34B8E347C7BF5DBB65A1D3，无WebDriver/Fixture标记。
- P1-08仍未关闭：远端停服一致性备份和按天递归删除backup/service-upgrades/staging属于新增副作用，安全审查要求用户明确确认停止/重启范围、保留天数、删除根目录与回滚方案；本轮未应用Agent补丁。
- GitHub等第三方CI尚未获得源码处理授权，当前只保留本地scripts/quality-gate.ps1；外部CI、安装包签名和产物上传不得静默启用。
- 阶段7.5-D继续保持“整改中”，阶段8、当前Release对外分发和原工作台退役继续冻结。
