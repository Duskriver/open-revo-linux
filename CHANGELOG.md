# 更新日志 (Changelog)

本文档记录 OpenRevo 项目的所有重大更新、功能特性、问题修复与硬件兼容性演进。
版本格式严格遵循 [语义化版本 2.0.0 (Semantic Versioning)](https://semver.org/lang/zh-CN/)。

---

## [Unreleased] - 待发布

### ✨ 新增 (Added)
- Phase 2：EC 满充/均衡/长效三档与双阈值；Fn/Win 锁、摄像头、关机 USB、来电自启及触摸板 EC 标记，总览与 CLI 共用语义动作，托盘加入电池预设。
- 可显式启用的 Q-Key 四档联动、供电门禁、去抖与模式回声过滤；Freedesktop 通知在独立线程投递，隐藏窗口不影响联动。配置 v3 新增默认关闭的 hotkeys_enabled。
- Linux 遥测来源、成功采样时间及未提供/故障/预热状态；界面标记长时间未更新的数据。
- XDG 配置 v3：曲线草稿、模式和自定义功耗；兼容 v1/v2 及旧 localStorage，旧场景以历史数据保留，启动和导入不自动应用硬件。
- 统一性能/散热能力与动作，GUI、托盘和 CLI 共用四档模式及自动保存；移除命名场景执行器、插件入口和待移植占位卡片。
- 完整包附带 `openrevo-cli`，通过会话 D-Bus 复用运行中的桌面控制实例，提供 mode、fan、cpu、battery、brightness、status 和恢复命令。
- Linux 功能与验证台账：16 类功能使用稳定 ID，分别记录实现、权限、软件、实机读回和生命周期证据。
- 固定开发工具链及版本检查入口 `pnpm check:toolchain`；本地和 CI 共用 Node、Rust、pnpm、Python 与 tauri-driver 版本源。

### ⚡ 优化 (Changed)
- Rust 后台线程维护租约，独立于状态查询与 WebView；关闭窗口隐藏至托盘；静态机型信息按进程缓存，缺失信息定时重试，动态读数与控制门禁继续实时读取。
- 更新至 React 19.3.1、TypeScript 7.0.2、Vite 8.3.5、Tauri JS/CLI 2.12.4 和 Rust Tauri 2.12.5；同步更新配套插件与锁文件，保留 zbus 5。
- Vite 使用 Oxc 压缩，保留现有 WebKit 构建目标；pnpm 配置迁入统一 workspace 文件，默认不允许依赖构建脚本。
- Cargo 检查和发行构建使用 `--locked`；CI 固定测试工具并保存工具链与 WebKit 版本元数据。

### 🐛 修复 (Fixed)
- 超时或故障控制不能通过迟到心跳重新续期；界面状态过期时暂停新调节，保留显式恢复入口，后台独立续租。
- 草稿保存与导航后的加载串行，防止在途保存被旧草稿覆盖；损坏或更新版本的配置拒绝覆盖。
- 声明深色原生控件配色，使 Mono/Cyber 的下拉框等控件与界面一致。
- 为 React 19 显式初始化导航计时器 ref；移除未配置 Tailwind 时无效的 CSS 指令并补齐 JSON 导入属性，消除新构建工具的迁移警告。

### 🛡️ 硬件与安全 (Security / Hardware)
- 新设备控制要求 device_controls_abi=1；电池能力/策略读取无效时禁写，保留共享位，失败回滚并重试；Win 锁切换脉冲失败不自动重发。未接入平台身份伪装或把木珑 MiniLED 通道套用到同方。
- 驱动改为 MECHREVO + Uniwill WMI 匹配，提供默认关闭的 `force` 厂商覆盖；传输 ABI v1、风扇 ABI v2，应用与驱动需配套升级。
- 模式/CPU 事务、供电与出厂上限校验迁入 Rust；内核白名单单字节读写，保留曲线与强冷失联恢复；删除 C 业务事务与宝龙达字典项。
- 新增只读 WMI 原始事件队列、用户态序号缺口诊断和 EC 水冷能力探测；本批补入模式键联动，BlueZ 尚未接入。
- 新驱动只完成离线验证；此前版本的实机记录不能证明本轮重构或新机型已验证。

---

<!-- 历史版本发布记录将归档于下方 -->
