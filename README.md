# INX 实施工作台（重构版）

面向项目实施与维护人员的桌面工作台，根据 `inxaiot-edge-workbench` 的实际能力重新进行产品、技术和工程设计。

当前已完成阶段7后端技术能力与真实环境门禁：双数据库、工作台共享数据、导入对账、SSH/SFTP、租约/fencing、恢复和三类部署双节点链路已经验证。代码级Review发现桌面端真实项目/登录、发布参数、HostKey、部署进度结果、历史和生产任务编排尚未闭环，现进入阶段7.5整改；浏览器Fixture不能作为桌面功能完成证据。

## 文档

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

打开 `http://127.0.0.1:1420/`。

## Tauri运行

```powershell
pnpm tauri dev
```

## 质量检查

```powershell
pnpm typecheck
pnpm lint
pnpm test
pnpm build
cargo check --manifest-path .\src-tauri\Cargo.toml
pnpm tauri build --no-bundle
```
