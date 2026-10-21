# 平台空 MAC 补齐与草稿差异回归

> 验证日期：2026-10-21。适用于本次未发布工作台源码；版本字段仍为 0.2.12，不表示已安装的 0.2.12 包包含修复。

## 1. 业务目标与规则

实施人员从项目平台取得尚未填写 MAC 的智能屏，采集地址后，可以直接在编辑资料中保存采集值，也可以直接进入“注册/更新到平台”检查并提交。列表只对尚未提交的实际差异标色；撤销差异后取消“资料有变更”。

1. 平台 MAC 为空、目标 IP 与观测一致且没有明确 MAC 修改时，编辑表单自动带入有效采集值。打开表单不保存草稿，保存后才形成待提交资料。
2. 已有非空平台 MAC、明确编辑或清空、变更 IP 等情况保留身份核对规则，不静默覆盖已有身份或沿用旧地址的观测。
3. 平台原 MAC 为空时，主动注册预检也尝试采集；采集成功且无冲突即可形成补齐差异。检查阶段不写平台，确认提交并回读成功后才显示更新成功。
4. 列表颜色、变更提示和注册差异统一按最新平台资料与有效拟提交值比较。MAC 大小写、分隔符的等价写法不算变化，普通字段仍按既有首尾空白和空值规则比较。
5. 清回平台原空值属于撤销差异。采集观测可继续作为普通颜色候选展示；再次编辑或主动预检可重新提供候选，不在后台自动生成草稿。
6. 同值草稿仍可保留原始内容及版本，防止提交期间的新编辑被错误清除。界面不能仅按草稿记录是否存在判定待更新。

## 2. 已确认原因与修改范围

原编辑器只为未注册本机屏自动带入采集 MAC；原注册预检只在新增、IP 或 MAC 修改时采集，平台空 MAC 且无草稿会被遗漏。变更提示又按草稿记录存在判断，正式本机数据库为并发保护保留同值草稿后，界面仍会显示变更。

修改工程为 `inxvision-assistance/inxaiot-desk-buddy`（Windows 运维工作台工程）。本次未修改小新应用、屏网络、屏端配置、NTP 或正式数据库结构。

| 已有源码文件 | 实现职责 |
| --- | --- |
| `src/features/smart-screen/ScreenLocalEditor.vue` | 自动预填候选、换地址撤销候选及保存反馈 |
| `src/shared/model/screenRegistration.ts` | 字段语义比较、有效草稿合成、注册差异与空 MAC 采集规则 |
| `src/shared/model/screenListProjection.ts`、`src/stores/smartScreens.ts` | 复用列表投影判断真实差异和变更提示 |
| `src/shared/model/screenMaintenance.ts`、`src/features/smart-screen/ScreenOperations.vue` | 关键资料差异保护与操作页提示保持一致 |
| `src-tauri/src/domain/smart_screen/registration.rs` | 正式后端的字段比较、草稿合成和并发冲突规则 |
| `src-tauri/src/infrastructure/smart_screen/registration.rs` | 平台空 MAC 采集、同地址可信历史候选、ADB 确认和身份冲突处理 |
| `src/dev-fixtures/screenFixtureAdapter.ts` | 开发测试适配器保留同值草稿版本，与正式保存语义一致 |

正式本机草稿存储、平台写入时的原始版本比较及提交后收敛机制保持原职责；没有通过直接删除同值草稿掩盖差异提示问题。

## 3. 自动回归

| 检查 | 实际结果及边界 |
| --- | --- |
| 前端全套 | 68 个文件、582 项全部通过 |
| 前端静态检查 | 13 个涉及文件的 ESLint 检查和 TypeScript 类型检查通过 |
| Rust 注册相关单元测试 | 12 项通过，含本轮规则及既有一体机注册用例 |
| 正式本机 SQLite 基础测试 | 13 项通过，含同值草稿保留版本、刷新及重开，以及旧提交结束时保留新的撤回编辑 |
| 既有注册实机测试文件 | 编译通过；本轮未运行该完整测试套件，真实操作范围见下一节 |

覆盖了非空平台 MAC 不自动替换、换 IP 不继承旧采集、非法 MAC 不预填、采集失败逐台确认、已有身份冲突、多网卡冲突和未确认 ADB 的阻断，以及无差异不阻挡设备操作。原型测试不替代正式 SQLite 的并发测试，也不等同于所有故障分支的实机验收。

## 4. 隔离桌面与实机闭环

运行当前源码构建的原生 Windows 调试程序，嵌入生产模式前端资源，使用正式设备及数据库适配器，未启用原型或远程调试。独立应用标识为 `com.inxaiot.desk-buddy.voice-gui-20261021`，生产数据目录 `D:\data\com.inxaiot.desk-buddy` 未访问或修改。

本轮使用 142 测试服务器上的两套专用临时数据库，只创建一条平台 MAC 为空的 10 寸测试屏资料。真实设备为测试说明中的 10 寸屏，通过无线网络上的 ADB 只读采集；采集结果的网卡来源不代表本次连接使用有线网络。

关键步骤均通过实际窗口点击完成；数据库取证只读，不代替界面提交。

| 桌面操作 | 界面与独立核验结果 |
| --- | --- |
| 获取/核对 MAC | 1/1 成功；详情平台 MAC 仍空，采集值有效 |
| 打开编辑并直接保存 | 无需点击“填入采集值”；列表 MAC 标橙色，并显示资料有变更 |
| 再编辑、清空 MAC 并保存 | 提示“与平台资料一致，没有待提交修改”；变更提示消失 |
| 刷新平台 | 无变更提示；原始同值草稿仍保留修改版本 2，基准及拟提交 MAC 均空；平台未写入 |
| 不再手填，直接注册预检 | 待更新 1 台，MAC 来源为本次采集，差异为空到采集值，确认提交可用；独立查询平台仍空 |
| 15:26:53 确认提交 | 更新成功；独立查询确认仅 MAC 变化，原主键及其余字段保持，共享结果成功 |
| 返回列表及详情 | 无待提交颜色或提示；平台 MAC 与采集值一致，草稿已收敛 |
| 再次注册预检 | 无需更新 1 台、待提交 0 台，来源为平台当前记录，不再要求连接设备 |

本轮涉及的两条共享操作为 MAC 采集和资料更新，均成功；原有 14 条本机历史完整保留，最终 18 条总任务全部成功。活动租约、待核实写入及结果保护均为 0。再次同值预检未增加平台写入。

隔离程序已退出。设备稳定序列身份、小新进程及系统启动标识均与本轮基线一致。本轮没有重启、安装或修改屏端配置；两台 4 寸屏未参与本次窄范围回归。欢迎词和登录 HTTP 问题继续按原记录后置。

15:28:45，清理程序重新确认本轮两套临时库的唯一标记与无活动任务状态后，仅删除这两套临时库；事后查询确认均不存在。本机隔离应用数据、会话、原历史和回归证据保留。

## 5. 本机证据

证据保存在本机忽略目录 `.review-tools/mac-draft-20261021/`，不随安装包分发；凭据和完整设备配置不复制到本文。

| 证据 | 内容 |
| --- | --- |
| [构建清单](../../.review-tools/mac-draft-20261021/build-manifest.json) | 隔离标识、真实构建及源码一致性 |
| [前端全套结果](../../.review-tools/mac-draft-20261021/frontend-regression.json) | 582 项结果 |
| [编辑自动带入](../../.review-tools/mac-draft-20261021/screenshots/03-editor-auto-mac.jpg)、[保存后列表](../../.review-tools/mac-draft-20261021/screenshots/04-list-pending-mac.jpg) | 自动候选和真实差异标色 |
| [清空后提示](../../.review-tools/mac-draft-20261021/screenshots/05-clear-mac-no-difference.jpg)、[刷新后列表](../../.review-tools/mac-draft-20261021/screenshots/06-refreshed-no-pending.jpg)、[数据库核对](../../.review-tools/mac-draft-20261021/gui-after-empty-save.json) | 同值无提示但保留版本 |
| [直接预检](../../.review-tools/mac-draft-20261021/screenshots/07-preview-live-mac-update.jpg)、[提交前核对](../../.review-tools/mac-draft-20261021/gui-before-submit.json) | 本次采集、可以提交且尚未写平台 |
| [更新成功](../../.review-tools/mac-draft-20261021/screenshots/08-platform-update-success.jpg)、[提交后核对](../../.review-tools/mac-draft-20261021/gui-after-submit.json) | 限定 MAC 更新及回读 |
| [再次无需更新](../../.review-tools/mac-draft-20261021/screenshots/11-repeat-preview-no-update.jpg) | 重复检查不产生写入 |
| [最终任务核对](../../.review-tools/mac-draft-20261021/gui-final.json)、[设备最终核对](../../.review-tools/mac-draft-20261021/device-final.json) | 历史、终态、设备无重启或进程变化 |
| [清理结果](../../.review-tools/mac-draft-20261021/cleanup-manifest.json) | 本轮临时两库已清理且确认不存在 |
