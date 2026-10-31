# OpenRevo Linux 移植开发版

此目录的 Linux 版本直接基于 OpenRevo 上游源码开发，沿用 GPL-3.0-or-later 许可、Tauri 2、Rust 核心、React、原有图标、卡片组件和主题系统。它是原生 Linux 程序，不需要 Wine。

社区维护仓库：[Duskriver/open-revo-linux](https://github.com/Duskriver/open-revo-linux)；上游：[faintonce/open-revo](https://github.com/faintonce/open-revo)。本 Fork 的 Linux 适配与发布配置于 2026-10-31 整理。Linux 安装包只从本 Fork 的 Releases 获取；上游 Windows 下载与更新渠道保持独立。

目前完成平台隔离、Linux 桌面窗口和标准系统接口接入，**尚未达到 Windows 版的硬件功能完整度**。Linux 界面只提供已经实现的操作。缺失数据表示为“未提供”，不会回退到 MockHAL。原来的 Windows 入口、UI 和驱动路径保留在条件编译分支中；本次没有在 Windows 上运行回归测试。

## 已实现的范围

| 功能 | Linux 实现 | 当前 GM5HG7A 实机状态 |
| --- | --- | --- |
| 原生窗口、托盘菜单、单实例 | Tauri / GTK3 / WebKitGTK 4.1 | 原生窗口通过 WebDriver 验证；托盘显示依赖桌面环境 |
| CPU 使用率、频率、内存 | `/proc/stat`、`/proc/cpuinfo`、`/proc/meminfo` | 已读取实机数据 |
| 温度、风扇转速、功率读取 | Linux hwmon | 温度与部分功率可读；未公开风扇转速 |
| 电池容量、健康度、供电状态 | Linux power_supply | 已读取实机数据 |
| 系统节能 / 均衡 / 性能档位 | power-profiles-daemon 或 tuned-ppd 的 D-Bus API | 三档均可枚举；已用授权进程验证当前档位的写入与读回 |
| 屏幕和单色键盘背光亮度 | 内核 backlight / leds + systemd-logind | 检测到屏幕背光；实际操作取决于活动会话授权 |
| 电池限充 | `charge_control_end_threshold` | 该实机内核未公开此接口；未进行实机限充测试 |
| 登录启动 | Tauri autostart / XDG desktop autostart | 已实现，未在本机开启 |
| 双主题、诊断导出 | 复用原有主题与组件；导出 JSON | 主题切换已验证 |

系统电源档位不等同于 OpenRevo 的 EC 办公 / 均衡 / 狂暴模式。通过 AMD PMF 或 CPU 驱动切换系统档位，并不能证明模式灯、机身按键和 OEM 功耗预设已经同步。

电源档位写入遵守系统 Polkit 策略。程序会非交互地查询当前进程是否有权限；如果明确被拒绝，界面保留当前档位显示并禁用切换按钮。自动化工具启动的进程可能不属于活动登录会话，应通过桌面应用启动器运行。移植版不修改系统授权规则。

## 构建与启动

Ubuntu / Debian 的主要构建依赖：

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf
```

另需 Rust stable 和 Node.js 20+，项目固定使用 pnpm 9.15.9。安装依赖后，从仓库根目录运行：

```sh
pnpm install --frozen-lockfile
pnpm linux:dev
```

开发工具将自动合并 `src-tauri/tauri.linux.conf.json`。Linux 使用原生窗口边框、可调整尺寸的可见主窗口，不依赖 Windows 的托盘抽屉定位逻辑。

构建内嵌前端的独立二进制与 Debian 安装包：

```sh
pnpm linux:build
```

结果位于 `src-tauri/target/release/open-revo` 和 `src-tauri/target/release/bundle/deb/`。在真实桌面会话中以普通用户运行。二进制依赖系统 WebKitGTK、GTK3 等动态库，并非跨所有发行版的静态可执行文件。本机 Ubuntu 26.04 的构建产物不保证能在更老的发行版运行；CI 使用 Ubuntu 22.04 构建。

## 无界面的诊断

核心探测工具不依赖 GTK、WebKit 或图形会话，只读取硬件信息：

```sh
cargo run --manifest-path src-tauri/Cargo.toml -p openrevo-core --bin openrevo-probe
```

已构建的主程序也支持：

```sh
./src-tauri/target/release/open-revo --linux-diagnostics
```

诊断 JSON 包括型号、主板、BIOS、CPU 和内核版本、传感器、系统电源服务与能力状态，不收集序列号。

探测工具的可选参数 `--verify-current-profile` 会把当前系统档位写回自身并读回验证，需要档位写入权限。默认不执行此操作。本次使用授权进程完成该验证，并确认前后的系统档位均为 `performance`。

## 验证

```sh
python3 scripts/check_vault_leaks.py
python3 -m unittest discover -s tests -p 'test_vault_gate.py' -v
pnpm linux:check
cargo clippy --manifest-path src-tauri/Cargo.toml -p openrevo-core --all-targets -- -D warnings
```

硬件门禁扫描前端、Tauri 与核心 crate 的业务源码，检查已登记 EC 地址、IOCTL、裸地址命名与直接数值访问。Windows 的 `scripts/check_vault_leaks.ps1` 调用同一检查器。该静态检查不能替代硬件协议审查。CI 同时执行 Linux 打包与 Windows 全目标编译检查；编译通过不等于 Windows 实机功能回归通过。

核心测试使用临时 sysfs / procfs 样本，不接触真实硬件。覆盖未知数据、单位转换、挂起设备、CPU 增量计数、电池限充参数与路径检查，以及背光设备发现。

原生窗口测试使用实际 Tauri IPC 和系统 WebKit，不是浏览器中的模拟后端：

```sh
cargo install tauri-driver --locked
# Ubuntu 22.04/24.04: sudo apt install webkit2gtk-driver xvfb
# Ubuntu 26.04: sudo apt install webkitgtk-webdriver xvfb
pnpm tauri build --debug --no-bundle
xvfb-run -a python3 tests/linux_webdriver.py
```

在无会话总线的 CI 中，使用 `dbus-run-session -- xvfb-run -a python3 tests/linux_webdriver.py`。测试覆盖真实状态进入窗口、主题切换、刷新、非法档位拒绝，以及档位权限和读回。有权限时只把当前档位写回当前值；无权限时验证系统拒绝写入且界面禁用操作；没有电源服务时跳过该项。测试不切换到其他档位，不改变登录启动项。

测试截图输出到被 Git 忽略的 `test-results/linux-window.png`。本次实机环境为 GM5HG7A、Ryzen 7 8845H、Ubuntu 26.04.1、内核 7.0.0-38-generic。

## Fork 开发与发布

本仓库在 `main` 开发，以 Linux 专用标签 `v<版本号>-linux.<序号>` 发布预览版。`origin` 指向自己的 Fork，`upstream` 指向原作者仓库。同步上游前先提交本地修改，再获取并审查上游变化。

发布前将 `src-tauri/tauri.linux.conf.json` 的 `version` 设置为与标签一致的语义版本（不含前缀 `v`）。推送对应标签后，Linux 发布流程会校验版本，运行 Linux 测试与打包、Windows 编译检查，并创建包含 `.deb`、源码归档和 SHA-256 校验的 **Release 草稿**。检查草稿后再公开发布。上游 Windows 发布与 Gitee 镜像任务只允许在原作者仓库运行。

## 后续硬件移植边界

| Windows 依赖 | Linux 对应工作 | 状态 |
| --- | --- | --- |
| UWACPIDriver / Win32 IOCTL / WMI | 为受支持的内核驱动建立 HAL 传输层，验证型号与固件，复用原有寄存器字典 | 待实现 |
| EC 风扇、功耗、限充、模式灯 | 通过上述传输层移植上游硬件业务逻辑 | 待实现 |
| ITE HID / BetterRGB | Linux hidraw / hidapi 设备枚举与数据包传输 | 待实现 |
| NVAPI / Windows 显卡 MUX | Linux 驱动与机型接口适配，分别声明读写能力 | 待实现 |
| WinRT BLE 水冷 | BlueZ D-Bus / GATT 传输层 | 待实现 |
| Win32 快捷键、前台进程、OSD | 分别适配 X11 与 Wayland 的桌面接口 | 待实现 |
| Windows 插件与自动更新 | Linux 插件权限、进程托管与包更新策略 | 待实现 |

本次没有加载或替换内核模块，也没有进行原始 EC / ACPI / WMI 调用。直接控制硬件时仍遵守仓库 AGENTS.md：寄存器与协议数据只登记在 HAL / vault，不能把 Windows 的寄存器写入直接套用到未核验的 Linux 机型上。

## 代码入口

- `src-tauri/crates/openrevo-core/src/linux/`：Linux 原生核心、sysfs / procfs、D-Bus。
- `src-tauri/src/linux.rs`：Tauri 命令、后台任务、窗口与托盘。
- `src/components/LinuxDashboard.tsx`、`src/types/linux.ts`：Linux 界面与 IPC 数据契约。
- `src-tauri/tauri.linux.conf.json`：Linux 窗口与 Debian 包配置。
- `tests/linux_webdriver.py`：原生窗口端到端测试。

标准接口参考：[Tauri Linux 构建依赖](https://v2.tauri.app/start/prerequisites/)、[内核 hwmon ABI](https://www.kernel.org/doc/html/latest/hwmon/sysfs-interface.html)、[PowerProfiles D-Bus API](https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html)、[systemd-logind](https://www.freedesktop.org/software/systemd/man/org.freedesktop.login1.html)。
