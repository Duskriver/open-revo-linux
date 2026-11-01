# M0 维护基线实施与验证

验证日期：2026-11-01（Asia/Shanghai）。[发展计划](linux-roadmap.md) 第一阶段已完成：建立 [16 类功能与验证台账](linux-feature-ledger.md)，更新依赖，并将 [开发工具链](development-toolchain.md) 固定到本地和 CI 共用的版本配置。

本次为 `e29c22402cab20bf701b9c87ce00bd67a02a524e` 基础上的未发布工作区构建，应用版本仍为 `0.9.3-linux.2`。它不是该公开版本原包的重复验证，也没有新增 Tag 或发布；以下结论仅对应本轮依赖及工具链变更。

## 已完成的变更

| 范围 | 实施结果 |
| --- | --- |
| 功能台账 | ORL-F001 至 ORL-F016；分开记录实现、权限、软件测试、实机 R0–R3、生命周期 L0–L2、机型范围、证据及下一步 |
| 开发工具 | Rust/Cargo 1.99.1、Node 24.22.0、pnpm 12.16.1、Python 3.14.4、tauri-driver 2.1.0；新增 `pnpm check:toolchain`，主检查与完整包构建均调用它 |
| 前端依赖 | React/React DOM 19.3.1、TypeScript 7.0.2、Vite 8.3.5、React 插件 6.1.2、Tauri JS API/CLI 2.12.4；更新类型、图标依赖和锁文件 |
| Rust 依赖 | Tauri 2.12.5、tauri-build 2.7.4、autostart 2.7.0、single-instance 2.5.5；同步解析必要传递依赖，保留 zbus 5.19.0 |
| 迁移适配 | React ref 显式初值、JSON 导入属性、Oxc 压缩；保留 `safari13` 构建目标，移除没有对应处理器的 Tailwind 指令；Tauri 重新生成已跟踪的 schema，默认权限集合未改变 |
| 安装与 CI | pnpm 设置统一放入 workspace 配置，精确依赖版本，默认不允许运行依赖构建脚本；Cargo 使用 `--locked`；CI 记录实际工具与 WebKit 包版本 |
| 目标系统边界 | 应用开发 CI 固定 Python；内核矩阵继续用 Ubuntu 22.04/24.04 的系统 Python 验证 DKMS 生成器；安装包不要求开发版 Python |

硬件业务逻辑、协议字典、内核驱动和设备匹配未修改。依赖迁移后的结果没有提高 OEM 实机验证等级，自动应用、后台 agent、RGB 等后续阶段能力仍未实现。

## 本轮检查结果

| 检查 | 结果 |
| --- | --- |
| `python3 tools/check_toolchain.py --check --with-driver` | 所有声明版本与实际命令匹配 |
| `pnpm install --frozen-lockfile` | 锁文件安装通过 |
| `pnpm check` | 字典门禁、34 项 Python、41 项 Rust 核心测试及 TypeScript 检查通过 |
| `make -C drivers/openrevo-ec test` | 6 项生成器测试、333 个风扇故障注入点、416 个模式/功耗/强冷故障注入点及相关场景通过 |
| `cargo clippy --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked -- -D warnings` | 通过，包括核心及 Tauri 宿主 |
| `pnpm build` | TypeScript 与 Vite 生产构建通过；最终构建无前述迁移警告 |
| `pnpm tauri build --debug --no-bundle -- --locked` | 原生调试应用构建通过 |
| 调试应用原生窗口回归 | 12 项中 10 项通过，2 项真实系统/平台档位写回按默认策略跳过 |
| `pnpm linux:build --output-dir <本轮验证目录>` | Rust release、前端压缩、Tauri Debian 中间包、GUI 与 DKMS 完整包构建通过 |
| `python3 packaging/build_linux_package.py --verify <完整包>` | 内容、源码一致性及安装元数据校验通过 |
| 完整包内应用的原生窗口回归 | 使用打包阶段提取的实际可执行文件，10 项通过、2 项真实写回跳过；覆盖 Oxc 压缩后的生产前端 |
| `pnpm audit --json` | 返回 0 项已知 npm 依赖漏洞；仅代表当时该审计源与锁文件的结果，不是 Rust/系统依赖审计结论 |
| 静态收尾检查 | CI YAML、Python 语法、文档链接及 `git diff --check` 通过 |

两次窗口回归均显式设置 `OPENREVO_TEST_HARDWARE_WRITES=0`，通过 `dbus-run-session` 与 Xvfb 执行。测试包含真实 IPC、状态读取、非法请求拒绝、导航、主题及调校/曲线夹具；生产包窗口测试没有运行安装维护脚本。源码构建、包校验和窗口回归均在本机完成，远端 GitHub Actions 尚未运行。

窗口环境为本机 Ubuntu 26.04.1，WebKitGTK 与实际提供 WebKitWebDriver 的 `webkitgtk-webdriver` 均为 `2.52.6-0ubuntu0.26.04.1`。日志含隔离会话的 portal/GVfs 提示、AppIndicator 弃用提示及 GTK scale-factor 断言，未导致回归失败；不将这些结果扩展为完整 Wayland/托盘兼容验证。测试时 OEM 通道报告设备忙，仍可显示标准系统状态；OEM 控制使用夹具验证，本轮不据此宣称重新验证了真实 EC 操作。

## 产物与追溯

本地验证目录为 `dist_release/m0-baseline-20261101-190159/`，包含 `checks.json`、各检查日志、工具版本、依赖审计结果、调试/生产窗口截图、完整包、SHA-256 和源码变更记录。该目录按现有规则不纳入 Git；本页保存公开摘要。

完整包名为 `OpenRevo_0.9.3-linux.2_amd64.deb`，SHA-256：

```text
44090e0d32e2382bb5120f0d262494a3c2e2455d490ca92eedc74069fcf8db3f
```

产物放在独立验证目录，已有发行目录根部的包予以保留。本轮只构建和运行提取后的应用，未安装该包、替换或加载模块，也未执行真实硬件调节。带载散热、重启/休眠、第二机型及目标发行版远端矩阵继续按台账待验证项推进。
