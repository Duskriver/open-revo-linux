# INVIOLABLE SYSTEM RULES & SAFETY GUARDRAILS (最高安全守则)

## 0. 惨痛历史事件警告 (WARNING: PREVENT RECURRENCE)
历史灾难：AI 在 Windows CMD 中执行带有转义引号的通配符删除命令：
`cmd /c "for /d %i in (path\??) do rmdir /s /q \"%i\""`
CMD 的转义解析缺陷将 `\"` 错误解析，导致 `rmdir /s /q` 接收到参数 `\`（驱动器根目录），直接递归抹除了整个磁盘（E:\），造成毁灭性的用户数据灭失。

## 1. 绝对禁止的操作 (STRICTLY PROHIBITED)
1. **严禁在 Windows CMD (`cmd /c`) 中使用任何包含引号转义 `\"`、`for /d` 变量与 `rmdir /s /q` 的组合语句**。
2. **严禁使用通配符（如 `*`、`?`）进行批量物理删除**。
3. **严禁任何未经验证路径边界的递归删除**。
4. **严禁在删除命令中传入相对路径、未展开变量或可能被拆解为 `\` 或 `/` 的参数**。

## 2. 必须执行的安全规范 (MANDATORY PRACTICES)
1. **删除优先移至回收站**：必须使用 Windows Shell API（`[Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($path, 'OnlyErrorDialogs', 'SendToRecycleBin')`），保留物理可恢复性。
2. **绝对字面量路径与边界拦截**：永久删除前必须调用 `Resolve-Path -LiteralPath`，且严格断言目标位于当前项目目录深处，绝对不能触碰磁盘根目录。
3. **高危删除必须暂停并向用户索取明确许可**。

## 3. 工作区项目目录与开发流规范 (WORKSPACE & DEV FLOW)

**架构与实施顺序以根目录 `linux_architecture_plan.md` 为准。** 开发前先核对该蓝图和末尾实施记录；与旧版 `docs/linux-roadmap.md` 等历史方案冲突时采用新蓝图。每批改动同步记录已实现、待实现和实际验证结果，不能把规划中的功能记作已支持。
1. **工程主体位置**：当前目录（`open-revo/`）为所有业务逻辑、驱动通信、前端 UI 与构建打包的核心工程根目录。
2. **归档隔离**：上级归档目录 `../bak/` 严禁修改与写入，仅作为历史逆向资料只读查阅。
3. **开发流规范**：本项目采用**单主干（`main`）+ Tag 驱动发布**的轻量开发流。所有提交直接推送到 `main`，正式版本严格以 Git Tag（如 `v1.2.0`）控制发布。
4. **Linux-only 开发入口**：
   - `pnpm linux:dev`：启动 Linux 桌面程序。
   - `pnpm check`：硬件字典边界、Python 测试、Rust 核心测试和 TypeScript 检查。
   - `pnpm linux:build`：构建包含 GUI 与 DKMS 驱动源码的完整 Debian 包，输出到 `dist_release/OpenRevo_<version>_amd64.deb`；仅构建，不安装或加载模块。
   - `python3 packaging/build_linux_package.py --verify <完整包路径>`：检查完整包内容与安装元数据。
   - `pnpm linux:driver:build`：仅供驱动开发的独立包构建入口，不作为默认用户发行产物。
   - `make -C drivers/openrevo-ec test`：无硬件的内核事务故障注入测试。
   - `make -C drivers/openrevo-ec`：仅编译内核模块，不加载。
   - 发布以 `src-tauri/tauri.conf.json` 为版本来源，产物归档至 `dist_release/`；公开发行一个完整 `.deb`，另附对应源码和 SHA-256 清单，不发布 Tauri 中间应用包。
5. **支持边界**：当前只维护 Linux。应用普通用户运行，Tauri 只承载桌面集成与 IPC，硬件逻辑在 `openrevo-core` 和 `drivers/openrevo-ec`。模块安装/替换和真实硬件调节应与离线构建测试分开确认。

## 4. 核心硬件资产字典与 HAL 强隔离守则 (HARDWARE ASSET DICTIONARY & HAL ISOLATION)
注意：本项目已完全开源（钥匙库配置 `config/ec_registry.json` 随仓公开），但为保证系统底层硬件调控的安全性、杜绝魔法数字散落导致的误写死机/烧机风险，以及跨机型自适应适配的一致性，必须严格执行以下 HAL 规范：
1. **严禁在业务代码中出现裸 EC 寄存器数字**：严禁在 HAL（`src-tauri/crates/openrevo-core/src/hal/` 和 `drivers/openrevo-ec/`）以外的任何业务文件内硬编码任何明文 EC 物理地址（如 `1844`、`1873`、`1958`、`1996` 等整数或十六进制字面量）。
2. **钥匙库唯一登记**：所有 EC 物理寄存器地址、掩码控制位、IOCTL 码与芯片定长协议帧，必须统一在 `src-tauri/config/ec_registry.json` 或 `src-tauri/crates/openrevo-core/src/hal/vault.rs` 中登记，由编译期构建脚本动态生成常量绑定。
3. **全栈引用规范**：业务逻辑层（功耗调度、设备开关、RGB灯控、水冷、硬件探测等）访问硬件时，必须统一引用 `crate::hal::vault::ec::*` 导出的常量别名（如 `ADDR_BATTERY_PROTECTION_MODE`、`ADDR_POWER_STATUS`），严禁越过钥匙库直接传裸数字。
4. **严禁裸地址命名与静态门禁拦截**：严禁使用诸如 `cur_1862`、`pre_1860`、`r1894` 等包含物理地址的变量名、注释或结构体字段，必须统一采用人类可读的业务语义命名（如 `cur_db_target_watts`、`pre_tgp_offset`）。所有开发代码提交前必须强制通过 `python3 scripts/check_vault_leaks.py` 扫描，该检查器是 `pnpm check` 与 Linux CI 的前置门禁。历史 PowerShell 外壳仅调用同一检查器。

## 5. 脚本编码与多语言环境安全守则 (INVIOLABLE SCRIPT ENCODING RULES)
1. **纯 ASCII 核心军规（Pure ASCII Policy）**：所有存放在 `scripts/` 及随项目打包分发的自动化维护、测试与编译脚本（`.ps1` 与 `.bat`），**必须 100% 采用 Pure ASCII（纯 ASCII 英文，字符范围严格限定在 `0x20..0x7E`、`\t`、`\r`、`\n`）编写**。严禁在脚本内部出现任何中文字符串、中文注释或全角符号。
2. **强制 Windows CRLF 换行（Strict CRLF Line Endings）**：所有 `.bat` 与 `.ps1` 脚本**绝对严禁单独使用 Linux 风格的 `\n` (LF) 换行**，必须全量统一采用标准的 Windows CRLF（`\r\n`）。在创建或修改任何 `.bat` 脚本后，必须立即调用专用工具脚本进行强制清洗与断言校验：
   ```powershell
   python scripts/ensure_ascii_crlf.py <目标脚本路径>
   ```
3. **交互非阻塞原则（Non-blocking Headless Execution）**：脚本严禁在无条件执行分支中盲目调用 `pause` 或 `Read-Host`，避免在非交互式后台调用、CLI 自动化或子进程管道中发生无限挂起卡死；历史 Windows 脚本需要管理员权限时通过 `WindowsBuiltInRole` 检查；Linux 安装入口使用明确的权限检查与发行版授权机制，GUI 不提权。
4. **用户引导外壳隔离（Safe Shell Wrapper）**：若确需向终端用户展示外壳引导，外壳批处理（`.bat`）首行必须显式声明 `@echo off`，所有提示文本必须严格收拢为纯 ASCII 英文，底层核心调度调用纯 ASCII + CRLF 的 `.ps1` 脚本。

## 6. 前端设计系统规范与样式收拢守则 (INVIOLABLE UI DESIGN SYSTEM TOKENS)
为保证 OpenRevo 界面质感高度统一、杜绝样式碎片化、确保多主题（尤其深色 Mono 黑白主题）无缝兼容，前端开发必须严格遵守以下规范：
1. **严禁在组件中硬编码行内样式 (No Hardcoded Inline Styles)**：严禁在 JSX/TSX 的 `style={{ ... }}` 中硬编码具体的颜色数值（如 `#ef4444`、`rgb(...)`）、硬编码边框或大段布局参数。所有颜色、表面层级、圆角与间距必须且只能使用 `src/index.css` 定义的 Design System Tokens（如 `var(--surface-overlay)`、`var(--border-medium)`、`var(--status-error)`、`var(--radius-lg)` 等）。
2. **优先复用预置原子与变体类名 (Component Class Reuse)**：
   - 交互按钮优先使用 `.toggle-btn`，强调态使用 `.active-indigo` / `.active-emerald` / `.active-cyan`，破坏性危险操作强制使用 `.toggle-btn.danger` 或 `.toggle-btn.btn-danger-solid`。
   - 系统设置行列统一使用 `.sys-rows`、`.quick-row`、`.sys-row-label`、`.sys-row-desc`、`.sys-row-controls`、`.sys-sub-panel`。
   - 弹窗与确认对话框统一使用 `.sys-modal-backdrop`、`.sys-modal-card`、`.sys-modal-header`、`.sys-modal-body`、`.sys-modal-actions` 等标准类名。
3. **新增组件样式必须沉淀至 CSS 并全量适配主题 (Theme Support & Centralization)**：若确实需要新增复合 UI 组件或布局规范，必须将 CSS 类统一定义在 `src/index.css` 中，且必须同步补充 `[data-theme="mono"]` 等主题层级的样式适配，严禁将零散样式孤立在单组件文件内造成视觉失控与主题断裂。
