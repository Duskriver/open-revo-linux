# OpenRevo Linux 移植开发版

此目录的 Linux 版本直接基于 OpenRevo 上游源码开发，沿用 GPL-3.0-or-later 许可、Tauri 2、Rust 核心、React、原有图标、卡片组件和主题系统。它是原生 Linux 程序，不需要 Wine。

社区维护仓库：[Duskriver/open-revo-linux](https://github.com/Duskriver/open-revo-linux)；上游：[faintonce/open-revo](https://github.com/faintonce/open-revo)。本 Fork 的 Linux 适配与发布配置于 2026-10-31 整理。Linux 安装包只从本 Fork 的 Releases 获取；上游 Windows 下载与更新渠道保持独立。

目前完成平台隔离、Linux 桌面窗口、标准系统接口及 GM5HG7A 的 Uniwill WMI/EC 通道接入，**尚未达到 Windows 版的硬件功能完整度**。2026-11-01 起与 Windows 共用导航栏、硬件状态卡片、性能模式选择器和功耗滑块，保留原版页面结构；未移植页面明确显示支持状态。缺失数据表示为“未提供”，不会回退到 MockHAL。原来的 Windows 入口、UI 和驱动路径保留在条件编译分支中；本次没有在 Windows 上运行回归测试。

## 已实现的范围

| 功能 | Linux 实现 | 当前 GM5HG7A 实机状态 |
| --- | --- | --- |
| 原生窗口、托盘菜单、单实例 | Tauri / GTK3 / WebKitGTK 4.1 | 原生窗口通过 WebDriver 验证；托盘显示依赖桌面环境 |
| CPU 使用率、频率、内存 | `/proc/stat`、`/proc/cpuinfo`、`/proc/meminfo` | 已读取实机数据 |
| 温度、风扇转速、功率读取 | Linux hwmon + Uniwill EC | 两只风扇转速实机可读；温度与功率保留 hwmon |
| 电池容量、健康度、供电状态 | Linux power_supply | 已读取实机数据 |
| 平台性能模式 | ACPI `platform_profile`，优先由系统电源服务协调 | 已检测到 `amd-pmf` 的 low-power / balanced / performance；写后核对平台模式 |
| 风扇强冷 / 恢复自动 | 原版 EC 强冷位；其他机型保留 hwmon | 实测约 2500/2600 → 5208/5401 RPM；正常释放及进程突然退出均恢复原强冷状态 |
| CPU PL1 / PL2 / PL4 | 原版寄存器字典、余量规则和自定义功耗开关 | 出厂三档表可读；写入与回滚通过测试，电池下禁止应用，尚未实机验证供电后的效果 |
| 机身办公 / 均衡 / 狂暴 | 原版模式位、模式索引、触发位和自定义标志 | 办公模式实机写入/读回通过；均衡/狂暴因未接原装电源而未实机切换 |
| 系统节能 / 均衡 / 性能档位 | power-profiles-daemon 或 tuned-ppd 的 D-Bus API | 三档均可枚举；已用授权进程验证当前档位的写入与读回 |
| 屏幕和单色键盘背光亮度 | 内核 backlight / leds + systemd-logind | 检测到屏幕背光；实际操作取决于活动会话授权 |
| 电池限充 | `charge_control_end_threshold` | 该实机内核未公开此接口；未进行实机限充测试 |
| 登录启动 | Tauri autostart / XDG desktop autostart | 已实现，未在本机开启 |
| 双主题、诊断导出 | 复用原有主题与组件；导出 JSON | 主题切换已验证 |

系统电源档位不等同于 OpenRevo 的 EC 办公 / 均衡 / 狂暴模式。通过 AMD PMF 或 CPU 驱动切换系统档位，并不能证明模式灯、机身按键和 OEM 功耗预设已经同步。

电源档位写入遵守系统 Polkit 策略。程序会非交互地查询当前进程是否有权限；如果明确被拒绝，界面保留当前档位显示并禁用切换按钮。自动化工具启动的进程可能不属于活动登录会话，应通过桌面应用启动器运行。移植版不修改系统授权规则。

## 共用前端与核心控制

`DashboardChrome` 从原版 `FullDashboard` 提取，两平台共用标题栏与横向导航，并共用 `BUILTIN_MODULE_SLOTS` 页面名称。`HardwareStatCards`、`ModeSelector` 和 `CpuPlSliderRow` 是同一套组件，样式收拢到 `src/index.css`，支持 Mono / Cyber。Linux 保留独立的数据与命令适配，避免挂载 Windows 专用页面时触发 NVAPI、注册表、Win32 和插件进程命令。缺失测量显示为未知；不再使用原状态卡中的示例频率、显存占用等回退数字。

- **平台性能模式**：从聚合 ACPI 接口或单个 platform-profile class 设备读取实际档位。系统电源服务存在时经该服务切换，再核对内核平台模式；服务不存在时才尝试写入已发现的 sysfs 接口。多个驱动且没有聚合接口时不任选一个。AMD PMF 的平台档位仍不能证明原版 EC 模式灯和 OEM 功耗表已经同步。
- **风扇**：发现 hwmon 自动调速通道后，可通过标准 `pwm*_enable=0` 请求全速；保存原自动模式编号，释放时恢复该编号。正在使用其他手动策略的风扇不可接管。前端轮询维持 10 秒租约，后端每 2 秒检查一次；租约过期或正常退出时恢复。设备消失/更换时不创建属性、不写新设备；其他控制器后来更改模式时尊重其设置。恢复失败保留记录并报告错误。进程被强杀、内核挂起时无法保证恢复，风扇可能保持全速。没有提供降低手动转速或用户态曲线控制。
- **功耗**：按驱动发现功耗通道，用原版滑块编辑并显式应用。上限必须由驱动提供；下限缺失时采用 1 W 的软件下限。拒绝未知通道、非有限数和越界输入，微瓦 / 瓦转换后写入，再读回实际值。驱动量化或其他策略覆盖会返回包含实际值的错误，前端重新刷新；不虚报成功、不循环覆盖系统策略。退出程序不会自动撤销用户应用的功耗值，也不会在下次启动时自动重放。

### GM5HG7A 原生 EC 通道

`src-tauri/src/hal/linux/openrevo_ec.c` 是小型 WMI 内核传输模块，调用本机 DSDT 中的 EC 邮箱方法，由固件自身的 EC mutex 串行化；不使用裸端口、`/dev/mem` 或 ACPI debug 写入。协议帧、GUID 与可访问寄存器白名单来自同一个 `src-tauri/config/ec_registry.json`，构建时生成 C 头文件。协议与 [Uniwill WMI 驱动参考实现](https://github.com/tuxedocomputers/tuxedo-drivers/blob/main/src/uniwill_wmi.c) 和本机 ACPI 方法交叉核验，使用 [Linux WMI 的设备实例接口](https://docs.kernel.org/wmi/driver-development-guide.html)。

当前只绑定 `MECHREVO / GM5HG7A / N.1.09MRO19`，已在 Linux `7.0.0-38-generic` 编译并加载。不按共享 GUID 猜测其他机型，也不冒充其他品牌 DMI。固件升级后需要重新核验并更新匹配范围。加载默认只读；安装脚本在当前已验证机型上启用白名单写入。单一进程独占设备，另一个控制器会得到 busy 错误。

Rust HAL 的 `tongfang.rs` 与 Windows 共用模式、强冷和功耗约束函数；`linux_ec.rs` 实现有错误返回、读回验证、失败恢复的控制序列。模式切换使用固件出厂功耗与自动风扇策略，保留灯光亮度、USB 充电位；自定义曲线复用 Windows 的 RAM 风扇表生成算法；没有调用 NVIDIA 的 Windows API。机身灯寄存器随模式写入，但灯光外观没有独立实测。

CPU 功耗不是假设的 TDP 表：本机读取办公 **35/35/70 W**、均衡 **65/65/100 W**、狂暴 **90/90/100 W**。自定义编辑要求原版的 `PL1 <= PL2`、`PL4 >= PL2 + 20 W`，并限制在出厂狂暴上限内。因此当前自定义 PL2 最高为 80 W；固件原生狂暴表仍保持不动。出厂表或自定义寄存器读回不能单独证明 CPU 的实际长期耗电或 SMU 限制，供电后的负载实测尚待完成。自定义写入前停用旧表，三项写完并读回后再启用；失败时恢复已写寄存器，恢复失败会明确报错。

系统供电状态与 EC 圆口供电标志必须同时通过，才允许均衡/狂暴或 CPU 自定义功耗。电池和仅 Type-C 供电时禁止这些操作。用户当前无法接电，所以本轮只实机验证办公模式、转速、强冷、恢复与进程退出保护；没有进行高功耗写入。

风扇继续由 EC 温控，强冷只改原版强冷位。前端失联 10 秒后后端释放；正常关闭和进程异常退出时，内核文件 release 也恢复原来的强冷位，同时保留模式位。内核挂起或硬件断连时无法保证恢复。自定义曲线现已接入，接电实测待完成。

安装当前内核模块（先安装当前内核 headers）：

```sh
make -C src-tauri/src/hal/linux
sudo python3 src-tauri/src/hal/linux/install.py
```

安装到 `/lib/modules/$(uname -r)/extra/openrevo`，添加专用 modules-load / modprobe 配置和 `70-openrevo-ec.rules`。udev 的 `uaccess` 仅向活动本地桌面会话授予设备访问权限，GUI 以普通用户运行。未修改 Polkit。Secure Boot 环境需要自行签名；此安装方式尚未集成 DKMS，内核升级后需要重新构建安装。已加载旧模块时应先退出程序并卸载旧模块，再安装新构建。

只读诊断及显式硬件冒烟检查：

```sh
cargo run --manifest-path src-tauri/Cargo.toml -p openrevo-core --example linux_ec_probe
# The next commands do change hardware; fan-smoke restores the previous state.
cargo run --manifest-path src-tauri/Cargo.toml -p openrevo-core --example linux_ec_probe -- --fan-smoke
cargo run --manifest-path src-tauri/Cargo.toml -p openrevo-core --example linux_ec_probe -- --office
```

其他机型继续使用标准 sysfs 控制，所有写入要求当前进程有设备权限。缺失 EC 模块或权限时会在诊断中说明原因，不回退到模拟数据。

### 自定义风扇曲线（接电实测待完成）

Linux 的电源管理页现在直接使用原版 `FanCurveEditor`：CPU/GPU 独立 8 点曲线、联动、SVG 拖动、滚轮/方向键和 1% 微调，以及 100/300/500/1000 ms 平滑过渡。编辑和保存草稿不会写硬件；草稿保存在本地，不在启动时自动应用。恢复按钮恢复启用前的完整策略，而不是猜测一个通用风扇预设。原版编辑器的样式已收拢到 `src/index.css` 的主题变量中，Windows 入口也使用同一个组件。

Windows 桌面入口及 core crate 的曲线类型、出厂表和插值函数共同引用 `hal/fan_curve.rs`；不另写一套 Linux 插值。保留原版的 16 档温度网格、低温 8°C/其他 5°C 迟滞、零转速首档、0–200 占空比缩放和 GPU 尾部控制规则。Linux 接口要求 8 个温度递增/占空比不下降的节点、22–100% 范围、95°C 达到 100%，后端与驱动再次校验。原版空曲线的机型表回退行为保留。

`fan_transaction.h` 在内核中执行完整请求：先备份表格，临时请求强冷，执行原版预锁存、整表写入和读回、提交锁存，再释放临时强冷。GPU 表最后三个物理槽是两个阈值和控制位，不能当作普通速度点；驱动单独转换它们，不写零控制位。曲线帧不允许指定寄存器地址，单字节接口也没有放开 RAM 表写入白名单。

独立风扇输出依赖原版 EC 自定义模式开关，这个开关同时管理 CPU 功耗。驱动保存并恢复开关和原功耗寄存器；首次从普通模式进入时，将当前模式的出厂功耗表填入自定义功耗槽，避免启用未初始化的零值。已有自定义功耗则沿用原值。曲线运行时需先恢复原策略才能另行调节功耗，切换机身性能模式也会先恢复曲线。

写入失败时恢复本次操作前的表；更新失败保留上一次有效曲线。原始备份一直保留到显式恢复、退出、进程关闭、租约到期、拔掉圆口电源或休眠。内核每 2 秒检查供电和 10 秒租约；恢复失败时保留备份、报告故障并尝试强冷与恢复。硬件/总线失效时不能保证写入成功，界面会显示错误。这些故障分支已通过内存 EC 故障注入验证，**尚未在实机写入曲线或验证带载散热效果**。

模块提供 `fan_curve_abi`、`fan_curve_active` 和 `fan_curve_fault` 只读状态。旧模块不具备整表接口时，界面允许编辑草稿并提示更新驱动。重新安装前退出 OpenRevo：

```sh
make -C src-tauri/src/hal/linux
make -C src-tauri/src/hal/linux test
sudo modprobe -r openrevo_ec
sudo python3 src-tauri/src/hal/linux/install.py
```

`make ... test` 编译并运行同一个事务实现的无硬件测试，覆盖逐个传输故障、读回不一致、恢复失败后重试、零控制位拒绝、供电与强冷冲突。测试不打开任何真实设备。当前通过 37 项核心测试、12 项原生窗口测试及 333 个传输故障位置检查，驱动已编译更新但未在实机启用曲线。Windows 端共享算法已做回归样例检查，尚未执行 Windows 实机回归。Linux 构建必须保留仓库中的 `ec_registry.json`，缺失时停止构建，不生成可写的零地址替代值。

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

原有 Windows 开发探针不随 Linux 安装包分发。在 Windows 上运行这些工具时，显式启用 `windows-probes`，例如 `cargo run --manifest-path src-tauri/Cargo.toml --features windows-probes --bin hardware_probe`。

已构建的主程序也支持：

```sh
./src-tauri/target/release/open-revo --linux-diagnostics
```

诊断 JSON 包括型号、主板、BIOS、CPU 和内核版本、传感器、系统电源服务与能力状态，不收集序列号。

探测工具的可选参数 `--verify-current-profile` 会把当前系统档位写回自身并读回验证，需要档位写入权限。默认不执行此操作。本次使用授权进程完成该验证，并确认测试前后的系统档位一致。

## 验证

```sh
python3 scripts/check_vault_leaks.py
python3 -m unittest discover -s tests -p 'test_vault_gate.py' -v
pnpm linux:check
cargo clippy --manifest-path src-tauri/Cargo.toml -p openrevo-core --all-targets -- -D warnings
```

硬件门禁扫描前端、Tauri 与核心 crate 的业务源码，检查已登记 EC 地址、IOCTL、裸地址命名与直接数值访问。Windows 的 `scripts/check_vault_leaks.ps1` 调用同一检查器。该静态检查不能替代硬件协议审查。CI 同时执行 Linux 打包与 Windows 全目标编译检查；编译通过不等于 Windows 实机功能回归通过。

核心测试使用临时 sysfs / procfs 和故障注入 EC 样本，不接触真实硬件。覆盖未知数据、单位转换、挂起设备、CPU 增量计数、电池限充、背光发现；新增平台档位白名单、风扇租约/恢复/热拔插/控制权变更、功耗范围/有限数/单位转换和越界路径拒绝。

原生窗口测试使用实际 Tauri IPC 和系统 WebKit，不是浏览器中的模拟后端：

```sh
cargo install tauri-driver --locked
# Ubuntu 22.04/24.04: sudo apt install webkit2gtk-driver xvfb
# Ubuntu 26.04: sudo apt install webkitgtk-webdriver xvfb
pnpm tauri build --debug --no-bundle
xvfb-run -a python3 tests/linux_webdriver.py
```

在无会话总线的 CI 中，使用 `dbus-run-session -- xvfb-run -a python3 tests/linux_webdriver.py`。测试覆盖真实状态进入窗口、主题切换、刷新、非法档位拒绝，以及档位权限和读回。有权限时只把当前档位写回当前值；无权限时验证系统拒绝写入且界面禁用操作；没有电源服务时跳过该项。测试不切换到其他实机档位，不改变登录启动项。另用仅限测试进程的 IPC 响应夹具验证风扇开启/恢复、功耗编辑/应用/读回的前端流程；这些夹具写入不会送往真实硬件，不能替代实机调节测试。

测试截图输出到被 Git 忽略的 `test-results/linux-window.png`。本次实机环境为 GM5HG7A、Ryzen 7 8845H、Ubuntu 26.04.1、内核 7.0.0-38-generic。

## Fork 开发与发布

本仓库在 `main` 开发，以 Linux 专用标签 `v<版本号>-linux.<序号>` 发布预览版。`origin` 指向自己的 Fork，`upstream` 指向原作者仓库。同步上游前先提交本地修改，再获取并审查上游变化。

发布前将 `src-tauri/tauri.linux.conf.json` 的 `version` 设置为与标签一致的语义版本（不含前缀 `v`）。推送对应标签后，Linux 发布流程会校验版本，运行 Linux 测试与打包、Windows 编译检查，并创建包含 `.deb`、源码归档和 SHA-256 校验的 **Release 草稿**。检查草稿后再公开发布。上游 Windows 发布与 Gitee 镜像任务只允许在原作者仓库运行。

## 后续硬件移植边界

| Windows 依赖 | Linux 对应工作 | 状态 |
| --- | --- | --- |
| UWACPIDriver / Win32 IOCTL / WMI | GM5HG7A 专用 WMI 内核传输，复用寄存器字典 | 当前 BIOS 已实现并实机验证；其他机型待核验 |
| EC 风扇、功耗、限充、模式灯 | 通过上述传输层移植上游硬件业务逻辑 | 强冷、模式与 CPU 功耗控制已接入；自定义曲线整表控制已接入；供电下实测和 EC 限充待完成 |
| ITE HID / BetterRGB | Linux hidraw / hidapi 设备枚举与数据包传输 | 待实现 |
| NVAPI / Windows 显卡 MUX | Linux 驱动与机型接口适配，分别声明读写能力 | 待实现 |
| WinRT BLE 水冷 | BlueZ D-Bus / GATT 传输层 | 待实现 |
| Win32 快捷键、前台进程、OSD | 分别适配 X11 与 Wayland 的桌面接口 | 待实现 |
| Windows 插件与自动更新 | Linux 插件权限、进程托管与包更新策略 | 待实现 |

本次没有加载或替换内核模块，也没有进行原始 EC / ACPI / WMI 调用。直接控制硬件时仍遵守仓库 AGENTS.md：寄存器与协议数据只登记在 HAL / vault，不能把 Windows 的寄存器写入直接套用到未核验的 Linux 机型上。

## 代码入口

- `src-tauri/crates/openrevo-core/src/linux/`：Linux 原生核心、sysfs / procfs、D-Bus。
- `src-tauri/src/linux.rs`：Tauri 命令、后台任务、窗口与托盘。
- `src/components/common/DashboardChrome.tsx`、`src/components/overview/HardwareStatCards.tsx`、`src/components/ModeSelector.tsx`、`src/components/tuning/CpuPlSliderRow.tsx`：两平台共用前端组件。
- `src/components/LinuxDashboard.tsx`、`src/types/linux.ts`：Linux 数据、命令与页面适配。
- `src-tauri/crates/openrevo-core/src/linux/controls.rs`：平台模式、强冷租约和功耗控制。
- `src-tauri/tauri.linux.conf.json`：Linux 窗口与 Debian 包配置。
- `tests/linux_webdriver.py`：原生窗口端到端测试。

标准接口参考：[平台性能模式](https://docs.kernel.org/userspace-api/sysfs-platform_profile.html)、[Powercap](https://docs.kernel.org/power/powercap/powercap.html)、[Tauri Linux 构建依赖](https://v2.tauri.app/start/prerequisites/)、[内核 hwmon ABI](https://www.kernel.org/doc/html/latest/hwmon/sysfs-interface.html)、[PowerProfiles D-Bus API](https://upower.pages.freedesktop.org/power-profiles-daemon/gdbus-org.freedesktop.UPower.PowerProfiles.html)、[systemd-logind](https://www.freedesktop.org/software/systemd/man/org.freedesktop.login1.html)。
