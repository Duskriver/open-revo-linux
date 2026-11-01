# Linux 开发工具链与依赖维护

更新日期：2026-11-01。本文描述仓库当前声明的开发、测试和构建基线；实际测试结果另见 [功能验证台账](linux-feature-ledger.md) 与对应版本记录。工具版本已写入配置，不等于所有环境或硬件行为已经验证。

## 固定版本与来源

| 工具 | 当前版本 | 版本来源与使用方式 |
| --- | --- | --- |
| Node.js | `24.22.0` | [`.node-version`](../.node-version)；本地版本管理工具按此文件选择，CI 的 setup-node 直接读取 |
| Rust / Cargo | `1.99.1` | [`rust-toolchain.toml`](../rust-toolchain.toml)，`minimal` profile，包含 `clippy`；rustup 在仓库内选择该工具链 |
| pnpm | `12.16.1` | [`package.json`](../package.json) 的 `packageManager`；`engines.pnpm` 与其一致 |
| Python | `3.14.4` | [`tests/toolchain.json`](../tests/toolchain.json)；用于开发检查、回归与 CI |
| tauri-driver | `2.1.0` | [`tests/toolchain.json`](../tests/toolchain.json)；仅原生窗口回归需要，使用精确版本安装 |
| WebKitGTK / WebKitWebDriver | 发行版匹配包 | 由系统包管理器提供；记录版本，不跨发行版硬钉同一版本 |

`package.json` 的 `engines.node` 与 `.node-version` 保持一致，工具检查器会检查这两份兼容性声明。CI 从上述文件读取版本，不再另写 Rust `stable`、Node 主版本或随时间变化的测试工具版本。Rust 的项目工具链文件与覆盖优先级见 [rustup 官方说明](https://rust-lang.github.io/rustup/overrides.html#the-toolchain-file)；若本地有 `RUSTUP_TOOLCHAIN` 或目录 override，检查实际生效版本。

当前应用依赖基线为 React / React DOM `19.3.1`、TypeScript `7.0.2`、Vite `8.3.5`、`@vitejs/plugin-react` `6.1.2`、Tauri JS API/CLI `2.12.4`，以 [`package.json`](../package.json) 和 [`pnpm-lock.yaml`](../pnpm-lock.yaml) 为准。Rust Tauri 在 [`Cargo.lock`](../src-tauri/Cargo.lock) 中锁为 `2.12.5`；Rust 清单的兼容版本要求与锁文件中的精确解析版本是两层约束，不能把清单版本当成所有传递依赖的精确版本。

保留 zbus 5 系列及现有 `blocking-api` / `async-io` 配置，当前锁定 `5.19.0`。本轮沿用已有系统 D-Bus 调用路径和同步 API，不把通信层的主版本迁移混入构建基线更新；相关声明见 [core Cargo 清单](../src-tauri/crates/openrevo-core/Cargo.toml)。

## 本地准备

先用已有版本管理工具安装并选择 `.node-version` 中的 Node，选择 `tests/toolchain.json` 中的开发 Python，使当前 shell 的 `node` 与 `python3` 分别指向它们。项目不限定版本管理器；仅创建版本文件不会让所有管理器自动切换。开发 Python 可放在用户工具目录，不替换发行版的 `/usr/bin/python3`。

在项目根目录确认命令来源，然后仅启用所选 Node 工具目录中的 pnpm 入口：

```sh
command -v node corepack python3
node --version
python3 --version
corepack enable pnpm
pnpm --version
rustup show active-toolchain
```

执行前应确认 `node` 和 `corepack` 来自当前选择的 Node 工具目录。`corepack enable pnpm` 只配置该 Corepack 安装旁的 pnpm 代理入口，项目内使用 `packageManager` 指定的版本；它不切换 Node、不升级项目依赖，也不需要对其他 Node 安装执行配置。若当前 Node 发行方式没有提供 Corepack，按 [Corepack 官方安装说明](https://github.com/nodejs/corepack#how-to-install) 补齐该工具目录后继续；入口行为见 [Corepack enable](https://github.com/nodejs/corepack#corepack-enable--name)。

若 `pnpm --version` 仍显示旧版本，先用 `command -v pnpm` 检查 PATH 中是否有旧入口遮挡，调整当前 shell 的工具选择。无需删除整个工具目录，也无需修改其他项目的包管理器配置。

安装 Ubuntu / Debian 构建与原生窗口回归所需的系统依赖：

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf \
  webkit2gtk-driver xvfb dbus-x11
```

WebKitGTK 与 WebKitWebDriver 应来自发行版匹配的软件包。GTK/WebKit 的系统版本影响原生窗口行为，应随测试结果记录；不将某台开发机的包版本写成所有发行版的硬要求。

工具准备好后安装锁定依赖并执行检查：

```sh
pnpm install --frozen-lockfile
pnpm check:toolchain
pnpm check
pnpm build
```

`pnpm check` 会先运行工具链检查，再运行硬件字典门禁、Python 测试、带 `--locked` 的 Rust 核心测试及 TypeScript 检查。`pnpm linux:build` 同样先检查工具链，Tauri 构建把 `--locked` 传给 Cargo，再构建完整 Debian 包。`pnpm linux:dev` 用于启动桌面开发程序。

## 工具核对与原生窗口回归

[`tools/check_toolchain.py`](../tools/check_toolchain.py) 只读取配置、查询工具版本并输出 JSON，不安装或升级工具。可单独查看声明，或核对当前 shell：

```sh
python3 tools/check_toolchain.py
python3 tools/check_toolchain.py --check
```

脚本检查 Node、pnpm、Rust、Cargo 和运行该脚本的 Python 精确版本，版本不符或命令缺失时失败。`pnpm check:toolchain` 等同于第二条命令。原生窗口回归还需要安装匹配的 tauri-driver；下例中的版本对应本次基线，更新时以 `tests/toolchain.json` 为准：

```sh
cargo install tauri-driver --version 2.1.0 --locked
python3 tools/check_toolchain.py --check --with-driver
pnpm tauri build --debug --no-bundle -- --locked
dbus-run-session -- xvfb-run -a python3 tests/linux_webdriver.py
```

tauri-driver 没有可用于此检查的 `--version` 参数；检查器从 PATH 中实际命令所在安装目录读取 Cargo 安装元数据，避免检查到另一份安装。原生窗口测试默认跳过真实硬件写回，UI 夹具不写硬件；硬件测试范围仍遵守 [Linux 验证说明](linux-port.md#验证)。

应用构建 CI 会保存 `test-results/toolchain.json` 与 `test-results/webkit-packages.txt`，分别记录实际工具链和发行版 WebKit 包版本。内核编译矩阵另外使用目标发行版的 `/usr/bin/python3` 运行 DKMS 生成器测试，保留其系统 Python 兼容性覆盖。它们提供复现线索，不证明不同桌面、发行版或 BIOS 的行为相同。

## Python 开发基线与安装包边界

Python `3.14.4` 是开发和回归环境的固定版本。当前测试依赖 Python 标准库，不需要另建 pip 依赖锁。CI 先读取 `tests/toolchain.json` 再安装指定 Python；安装包不执行开发工具链检查器。

完整 Debian 包继续依赖发行版的 `python3`，安装维护入口使用系统 Python。DKMS 字典生成、包维护脚本须保持声明支持的发行版 Python 兼容性；不能因为开发环境升级，就向安装脚本引入只在开发 Python 可用的语法或模块。构建工具版本固定与目标系统兼容性测试是不同要求。

## 依赖更新流程

1. 先确定目标版本及维护范围，阅读官方迁移说明；将依赖迁移与硬件策略改变分开。React 更新同步检查 DOM 与类型包；Vite 更新同步检查 Node、插件、构建目标和原生 WebKit 行为。
2. 更新相应版本源；Node/pnpm 同步更新 `engines`，Python/tauri-driver 更新 `tests/toolchain.json`。pnpm 项目设置统一在 [`pnpm-workspace.yaml`](../pnpm-workspace.yaml)，当前使用 `engineStrict: true`、hoisted 布局和显式的 `allowBuilds: {}`；新增依赖需要安装脚本时先审核并精确声明。pnpm 12 的设置归属见 [官方配置文档](https://pnpm.io/settings)。
3. 使用选定的工具链有意更新清单与锁文件，审查直接/传递依赖变化；保留 `pnpm-lock.yaml` 与 `src-tauri/Cargo.lock`。常规验证使用 `pnpm install --frozen-lockfile` 和 Cargo `--locked`，不在测试阶段顺带刷新依赖。`--locked` 的锁文件约束见 [Cargo 官方说明](https://doc.rust-lang.org/cargo/commands/cargo-build.html#manifest-options)。
4. 运行对应检查：`pnpm check`、驱动故障测试、前端构建、带 `--locked` 的 Rust 检查、原生窗口及完整包校验。依赖升级涉及 UI 时检查 Mono/Cyber、最小窗口、表单与曲线操作；涉及系统库时记录实际发行版/桌面组合。
5. 记录实际执行命令、结果、跳过项、工具元数据与产物版本。更新本页的基线摘要和相关验证台账，不能将尚未运行的检查写成通过。按 `main` 加 Tag 流程发布；本地构建、模块安装与实机调节保持独立。

本次 Vite 配置使用 Oxc 压缩并显式保留浏览器构建目标。Vite 8 转向 Rolldown/Oxc 且默认浏览器目标有变化，因此需要审查配置并执行原生窗口回归，见 [Vite 8 迁移指南](https://vite.dev/guide/migration)。React 19 要求现代 JSX 转换，类型更新也涉及 ref；仓库使用 `react-jsx`，已显式初始化相关 `useRef`，后续迁移可参照 [React 19 升级指南](https://react.dev/blog/2024/04/25/react-19-upgrade-guide)。这些说明解释实现选择，不代替当前改动的测试报告。

## 单一发行版本来源

发行版本唯一来源是 [`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json) 的 `version`，当前为 `0.9.3-linux.2`：

- Vite 读取它生成 UI 版本，附带 Git 提交短哈希；附加哈希是构建标识，不修改 Debian 发行版本。
- 完整包、DKMS 源码目录、安装维护元数据与 `dist_release/OpenRevo_<version>_amd64.deb` 使用该发行版本。
- Release Tag 必须等于 `v` 加发行版本。当前预览发布工作流仍要求 Linux 预览格式，见 [发布工作流](../.github/workflows/linux-release.yml)；正式版本通道需独立实现。

`package.json` 和 Rust crate 当前保留基础版本 `0.9.3`，它们是包/crate 元数据，不是另一份发行版本来源。更新发行版本时检查相关元数据是否需要同步；工具链检查器目前负责开发工具版本一致性，不声称已经验证所有应用版本字段。依赖升级本身不自动创建 Tag、安装应用或替换内核模块。
