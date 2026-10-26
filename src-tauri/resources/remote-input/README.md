# 屏端辅助文字组件

实施人员在工作台查看单屏画面、选择输入框后发送文字。Android 7～9 使用已有系统按键方式插入文字和退格；Android 10及以后使用本目录的小型 Java 辅助程序替换当前输入框内容或清空。两种方式由读取到的系统 SDK 版本选择，失败不会自动改用另一通道。已经实测的兼容事实为 Android 8.1 和 Android 10；其他固件仍需对应现场验收。

此组件随工作台内嵌，不修改小新应用、不安装 APK、不设置输入法、不使用剪贴板。`focused-text.jar` 是预编译 DEX 文件容器，工作台用户不需要 Java 或 Android SDK。工作台以内容 SHA-256 命名，必要时通过 ADB 标准输入传到屏端 `/data/local/tmp`，读取校验后复用；文件中只有程序代码，没有账号、密码或截图。截图仍只在内存传输。执行后不保留常驻服务。

## 输入规则

- Java 类 `FocusedText` 只接受 `chat.xiaoxin.app` 中唯一、可见、启用、可编辑、已有输入焦点并支持设置文字动作的节点。
- Java 类 `TextProtocol` 接收标准输入二进制协议：`INX1` 四字节标识、设备起始毫秒与截止毫秒（各8字节大端）、动作（1字节，1替换、2清空）、正文长度（2字节大端）、正文。接收后要求 EOF。工作台先编码为无换行 Base64 ASCII，经 ADB 标准输入传输，设备端通过 `base64 -d` 直接管道解码，避免 Windows ADB 文本输入通道把二进制 `0x1a` 截断；JAR 传输使用同一编码，解码后校验完整文件哈希。
- 替换正文限制为1～128字节可打印 ASCII；清空使用独立动作且正文为零字节，不把误提交的空文本当作清空。
- Java辅助程序的正文及其 Base64 编码不放入进程参数、日志、任务计划或临时文件。仅输出固定 `XX_TEXT_*` 状态，不输出节点内容或异常正文。禁止使用旧 `UiObject.setText/getText`、层级导出和带正文的断言。Android 7～9 的系统按键通道也不把正文放入本机 ADB 参数，但设备端 `input text` 进程参数会短暂承载正文。
- 设备 `/proc/uptime` 与 Java `SystemClock.elapsedRealtime` 用于限制4秒发起窗口；过期、时钟不匹配或异常长未来期限拒绝操作。最后一次期限检查紧贴系统节点动作调用。
- 普通停止会等待在途动作收敛；未知结果不重发，并等待设备期限及框架响应余量。已被系统接收的输入无法保证撤销；返回成功仅表示系统接受此次设置动作，登录结果仍以屏端页面为准。

## 开发构建与检查

在工作台仓库根目录执行：

```powershell
& ./src-tauri/resources/remote-input/build.ps1 -Sdk 'D:\env\AndroidSdk'
```

构建使用 JDK 17、Android SDK 30 的 `android.jar/uiautomator.jar/android.test.base.jar` 与 Build Tools 35.0.1 的 D8。源码按 Java 8 编译、最低DEX API24；不依赖外部 Java 库。脚本先在本机执行纯协议测试，再创建固定 ZIP 时间戳的 JAR。每次使用独立构建目录，避免已删除的旧类残留。

修改源码后，应同步提交 `focused-text.jar`、`focused-text.sha256` 与 `source-manifest.json`。清单记录源码和产物哈希；Rust 单测校验内嵌JAR和预期哈希一致。构建中间文件位于仓库 `.review-tools/remote-input-build`，不使用工作台实例数据目录。

实机验收须分别验证账号框和密码框、常用半角符号及字面 `%s`、清空/退格、停止和超时不重发；不能仅凭测试运行器输出 `OK` 判断成功。
