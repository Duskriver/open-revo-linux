# 更新日志 (Changelog)

本文档记录 OpenRevo 项目的所有重大更新、功能特性、问题修复与硬件兼容性演进。
版本格式严格遵循 [语义化版本 2.0.0 (Semantic Versioning)](https://semver.org/lang/zh-CN/)。

---

## [Unreleased] - 待发布

### ✨ 新增 (Added)
- Linux 遥测来源、成功采样时间及未提供/故障/预热状态；界面标记长时间未更新的数据。
- XDG 曲线草稿配置 v1、旧 localStorage 草稿迁移、JSON 导入导出及上一份有效配置备份；保存和导入均不自动应用硬件。
- Linux 功能与验证台账：16 类功能使用稳定 ID，分别记录实现、权限、软件、实机读回和生命周期证据。
- 固定开发工具链及版本检查入口 `pnpm check:toolchain`；本地和 CI 共用 Node、Rust、pnpm、Python 与 tauri-driver 版本源。

### ⚡ 优化 (Changed)
- 状态查询与主窗口控制续租分离；静态机型信息按进程缓存，缺失信息定时重试，动态读数与控制门禁继续实时读取。
- 更新至 React 19.3.1、TypeScript 7.0.2、Vite 8.3.5、Tauri JS/CLI 2.12.4 和 Rust Tauri 2.12.5；同步更新配套插件与锁文件，保留 zbus 5。
- Vite 使用 Oxc 压缩，保留现有 WebKit 构建目标；pnpm 配置迁入统一 workspace 文件，默认不允许依赖构建脚本。
- Cargo 检查和发行构建使用 `--locked`；CI 固定测试工具并保存工具链与 WebKit 版本元数据。

### 🐛 修复 (Fixed)
- 超时或故障控制不能通过迟到心跳重新续期；状态过期时停止续租，保留显式恢复入口。
- 草稿保存与导航后的加载串行，防止在途保存被旧草稿覆盖；损坏或更新版本的配置拒绝覆盖。
- 声明深色原生控件配色，使 Mono/Cyber 的下拉框等控件与界面一致。
- 为 React 19 显式初始化导航计时器 ref；移除未配置 Tailwind 时无效的 CSS 指令并补齐 JSON 导入属性，消除新构建工具的迁移警告。

### 🛡️ 硬件与安全 (Security / Hardware)
- 保持既有硬件控制、机型/BIOS 白名单和恢复策略；本轮离线回归不提高真实硬件验证等级。

---

<!-- 历史版本发布记录将归档于下方 -->
