# OpenRevo for Linux：功能、构建与验证

本分支以 Linux 为唯一目标，保留 [OpenRevo 上游](https://github.com/faintonce/open-revo) 的来源与 GPL-3.0-or-later 许可。技术栈为 Tauri 2、Rust、React 和 TypeScript，普通用户运行 GUI。

当前 main 已完成 Linux 入口与核心整理，并接入新的内核事务协议。`0.9.3-linux.2` 将 GUI 和 DKMS 驱动源码合并为一个完整 Debian 包，已在维护者本机完成安装与验证。`v0.9.3-linux.1` 预览包早于这些改动。阶段记录见 [linux-refactor.md](linux-refactor.md)。

## 功能与验证范围

| 功能 | 实现与边界 |
| --- | --- |
| 原生窗口、托盘、单实例 | Tauri / GTK3 / WebKitGTK；托盘取决于桌面环境 |
| CPU、内存、温度、风扇、电池 | procfs、sysfs、hwmon；缺失数据保持未知，不回退 MockHAL |
| 系统/平台性能档位 | 优先经 power-profiles-daemon 或 tuned-ppd 协调，写后核对读回；遵守系统授权 |
| 屏幕及单色键盘背光 | backlight / leds + systemd-logind；需要活动会话授权 |
| 电池限充 | 使用内核 `charge_control_end_threshold`；当前 GM5HG7A 未公开该接口 |
| OEM 模式、强冷、CPU 功耗 | GM5HG7A 专用 WMI 内核模块，使用版本化事务接口 |
| 自定义风扇曲线 | 八点草稿、显式应用、读回、恢复、租约及故障保护；带载实测未完成 |
| 主题、自启动、诊断 | 保留 Mono / Cyber、XDG 登录启动和 JSON 导出 |
| RGB、水冷、GPU MUX、插件 | 尚未实现 Linux 控制，页面明确标注支持状态 |

系统档位、OEM 模式和 CPU 功耗限制是不同控制层。系统服务管理平台档位时，应用不另外争抢同一个 sysfs 策略。寄存器读回正确不等于 CPU 实际耗电、温度或模式灯外观已验证。

### 已有实机记录与新驱动状态

当前仅由项目维护者在本机使用。2026-11-01 已安装 `0.9.3-linux.2` 完整包，应用与 DKMS 同版本、设备权限正常，安装状态为 `ready`，无需重启。安装后窗口回归 10 项通过；5 秒强冷达到 5085/5210 RPM 并正常释放，当前狂暴模式及 90/90/100 W 功耗保持不变。详情见 [本机安装验证记录](linux-release-notes.md#验证边界)。

下述较早记录来自同日合并安装包之前的应用与事务驱动，用于补充控制行为的验证范围。

历史实机环境为 MECHREVO / GM5HG7A / BIOS `N.1.09MRO19`、Ryzen 7 8845H、Ubuntu 26.04.1、Linux `7.0.0-38-generic`。旧驱动下已读取真实状态，验证办公模式、强冷和恢复；风扇约从 2500/2600 RPM 升至 5208/5401 RPM。

出厂功耗表曾读到办公 35/35/70 W、均衡 65/65/100 W、狂暴 90/90/100 W。自定义仍保持 PL1 <= PL2、PL4 >= PL2 + 20 W、且不超过出厂上限，因此该机当前自定义 PL2 最高 80 W。原厂狂暴表不被改写。

2026-11-01 经确认在上述机器安装并加载配套 DKMS 事务驱动，同步安装重构后的应用。新驱动连续 5 次办公模式写入及读回通过；5 秒强冷时两只风扇读数为 5183/5135 RPM，释放控制后强冷关闭，模式、出厂功耗和其他稳定控制字段恢复一致，故障标志未触发。旧模块、配置和旧应用文件均保留校验备份。

实测发现固件会在模式提交后消费静音请求标志，已据实机失败日志修正最终读回规则：仅允许该标志由请求值变为空闲值，立即写后读回、其他字段及传输错误仍严格检查。对应成功、异常与回滚场景已纳入故障测试。接电负载功耗、均衡/狂暴的性能效果及自定义曲线的带载散热仍未验证。

同日经额外确认完成接电读回：均衡 65/65/100 W、狂暴 90/90/100 W、自定义功耗 35/35/70 W，结束后恢复测试前的均衡模式、关闭自定义功耗和强冷，出厂表未改变。这是控制事务验证，不是压力负载或实际耗电验证。

## 架构与控制边界

- `src/main.tsx`：唯一 Linux 前端入口。
- `src/lib/linuxApi.ts`、`src/hooks/useLinuxBackend.ts`：类型化命令、轮询、写后刷新和状态处理。
- `src-tauri/src/linux.rs`、`src-tauri/src/linux/commands.rs`：窗口、托盘、生命周期与稳定 IPC 名称。
- `src-tauri/crates/openrevo-core/src/linux/`：系统发现与策略协调。
- `src-tauri/crates/openrevo-core/src/hal/`：协议编码、风扇算法、读取与严格字典生成器。
- `drivers/openrevo-ec/`：独立 WMI 驱动、模式/功耗事务和无硬件故障测试。
- `packaging/`：完整 Debian 包构建、校验与 DKMS 维护脚本；独立驱动包保留为开发入口。

EC 物理地址、掩码和帧布局由 `src-tauri/config/ec_registry.json` 登记，Rust/C 构建阶段检查格式、范围和布局。新模块不接受用户指定地址的单字节写入，只接收版本化的模式、功耗、强冷和曲线请求。

驱动只匹配上述型号和 BIOS，加载默认只读；安装包为此明确匹配的驱动配置写入开关。通过 udev `uaccess` 授予活动本地会话设备访问权。GUI 不提权，不修改系统 Polkit 策略。

模式和功耗事务在内核中持锁执行，检查供电及出厂上限，提交后读回；失败恢复完整前状态，恢复失败保留备份并请求强冷、重试。强冷只改变其控制位，保留同一寄存器中的模式位。模式/功耗成功后仍保持到用户再次修改；不因关闭窗口而自动撤销。

自定义曲线沿用已有帧协议和插值规则，应用要求系统供电与 EC 圆口供电同时满足。退出、租约过期、圆口断电、休眠时恢复原策略；硬件或总线失效时不能保证恢复写入成功，界面会显示错误。曲线草稿可离线编辑保存，启动时不自动应用。

新应用要求匹配的 `control_abi`。旧、未知或无法读取协议版本的模块仅用于监测，控制页面提示升级；不保留旧单字节写入兼容路径。应用与模块应配套更新。

## 构建与启动

Ubuntu / Debian 的主要依赖：

```sh
sudo apt install build-essential pkg-config libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev patchelf
```

开发与回归工具固定为 Rust 1.99.1、Node.js 24.22.0、pnpm 12.16.1、Python 3.14.4；窗口测试使用 tauri-driver 2.1.0。版本来源、安装和更新流程见 [开发工具链](development-toolchain.md)，功能实现及真实验证等级见 [功能台账](linux-feature-ledger.md)。安装包使用发行版提供的系统 Python，开发版本要求不影响终端用户安装。

```sh
corepack enable pnpm
pnpm install --frozen-lockfile
pnpm check:toolchain
pnpm linux:dev
pnpm linux:build
```

版本、窗口与应用打包配置统一在 `src-tauri/tauri.conf.json`。`pnpm linux:build` 先构建 Tauri 应用，再合入 DKMS 源码和维护脚本，最终输出 `dist_release/OpenRevo_<version>_amd64.deb`。`src-tauri/target/release/bundle/deb/` 中的 Tauri 应用包是中间产物，不单独分发。构建不会安装或加载模块。程序依赖系统 GTK/WebKit 动态库；本机新发行版构建不保证能在更老系统运行，应用 CI 使用 Ubuntu 22.04 构建。

## 完整 Debian 包安装与升级

用户只需安装一个完整包，其中包含 GUI、机型驱动源码、字典快照、生成头、维护脚本和系统配置。联网运行 APT，让包管理器安装 GUI 运行库、DKMS、Python、kmod、udev、编译工具与内核 headers 依赖。已安装的 headers 可通过 `linux-headers` 虚拟包满足依赖，避免仅因缺少通用元包而额外升级内核；没有 headers 时，APT 安装发行版通用 headers 元包。安装命令：

```sh
sudo apt install ./OpenRevo_0.9.3-linux.2_amd64.deb
```

若包位于项目的 `dist_release/`，在该目录执行上述命令，或为文件名加上对应路径。旧的拆分包 `openrevo-ec-dkms` 由包关系自动替换，无需另装第二个驱动包。

当前运行内核有匹配 headers、机型/BIOS 在支持范围内且系统允许加载模块时，安装程序通过发行版 DKMS 机制构建并自动启用驱动。标准系统监控本身不依赖 OEM 模块。发行版通用 headers 依赖不能保证覆盖自编译、第三方或其他非标准内核；这种情况仍需先安装与运行内核精确匹配的 headers。

后续内核升级使用发行版 DKMS 机制重新构建，并遵循系统的模块签名配置。Secure Boot 下可能需要完成签名和 MOK 登记；参见 [Ubuntu 官方说明](https://documentation.ubuntu.com/security/security-features/platform-protections/secure-boot/)。安装脚本不能代替用户完成固件中的 MOK 确认。

已加载模块与新模块的 `srcversion` 相同时保留当前模块；不同时仅尝试正常卸载和加载。正在使用的旧模块不会被强制卸载，无法切换时会明确提示重启；重启前不能把旧模块当作新驱动使用。安装状态与后续处理提示记录在 `/var/lib/open-revo/driver-status.json`。在 chroot 或容器内构建安装镜像时，不会操作宿主机模块或设备。

缺少当前内核 headers、等待 MOK 或重启，以及不受支持的机型，会记录 `ready=false` 和处理提示，同时完成应用安装，保留标准系统监控。实际 DKMS 编译失败或发现手工旧模块冲突则会报告安装配置错误，需要处理对应问题。应用安装完成不等于 OEM 控制已经就绪。

处理提示中的问题后，配置失败的包可执行 `sudo dpkg --configure open-revo`。已完成配置、但驱动处于等待状态的包需要显式重试（版本号与已安装版本一致）：

```sh
sudo /usr/lib/open-revo/driver-setup configure 0.9.3-linux.2
```

`driver-status.json` 记录上次配置结果，不是持续更新的运行状态。

旧的手工安装与旧的拆分 Debian 包不同。手工安装可能留有 `/lib/modules/<kernel>/extra/openrevo/openrevo_ec.ko` 或其压缩副本。安装程序会报告冲突并停止配置，不擅自删除文件。先记录 `modinfo -n openrevo_ec` 和 `dkms status`，备份并处理旧副本，再重新配置完整包；迁移应保留可恢复备份和回退步骤。

卸载通过发行版包管理器处理：注销本包对应 DKMS 版本，不强制卸载当前模块；已加载模块在重启后退出。GUI 始终以普通用户运行。

### 仅供开发的驱动构建

驱动源码和无硬件测试仍可独立使用。以下命令不加载模块：

```sh
make -C drivers/openrevo-ec test
make -C drivers/openrevo-ec
pnpm linux:driver:build
```

最后一项生成开发用的 `dist_release/openrevo-ec-dkms_<version>_all.deb`，不属于默认 CI 上传和公开分发产物。构建阶段目录保留在 `dist_release/`，不自动递归清理。

## 无界面诊断

```sh
cargo run --manifest-path src-tauri/Cargo.toml -p openrevo-core --bin openrevo-probe
./src-tauri/target/release/open-revo --linux-diagnostics
```

诊断包含机型、BIOS、内核、传感器、系统服务、权限与控制协议状态，不采集序列号。不要为了运行 GUI 而使用 sudo。

## 验证

```sh
pnpm check
make -C drivers/openrevo-ec test
cargo clippy --manifest-path src-tauri/Cargo.toml -p openrevo-core --all-targets -- -D warnings
cargo check --manifest-path src-tauri/Cargo.toml --workspace --all-targets --locked
pnpm tauri build --debug --no-bundle
dbus-run-session -- xvfb-run -a python3 tests/linux_webdriver.py
```

原生窗口测试需要 `tauri-driver`、`WebKitWebDriver` 和 Xvfb。默认不执行真实硬件写入，两个当前档位写回检查跳过；UI 夹具不会把控制请求送给真实硬件。只有在明确授权后设置 `OPENREVO_TEST_HARDWARE_WRITES=1` 才运行该两项检查。它们也不能代替接电负载或异常恢复实测。

`linux_ec_probe` 示例默认只读。经明确确认后可用 `--office` 验证办公模式、`--fan-smoke` 验证 5 秒强冷及释放恢复，或 `--ac-smoke` 验证均衡/狂暴和 35/35/70 W 功耗并恢复均衡。接电测试只接受原装电源、原厂均衡模式、自动风扇和已核验的出厂表；即使检查失败，也会尝试恢复并报告恢复结果。成功应用的模式和功耗不会仅靠进程退出自动撤销，因此该示例显式恢复均衡模式。

CI 包含核心/字典/打包脚本测试、前端检查、原生窗口、完整 Debian 包，以及 Ubuntu 22.04/24.04 headers 的内核模块编译。默认仅构建并上传一个完整 `.deb`，通过 `python3 packaging/build_linux_package.py --verify <完整包路径>` 检查包内容与安装元数据；不再检查或发布 Windows 构建，也不安装或加载测试机模块。

## 发布

继续使用 main 与 Tag 发布流程。Linux 预览标签为 `v<版本>-linux.<序号>`，必须匹配 `src-tauri/tauri.conf.json` 的版本。发布流程只接受一个 `OpenRevo_<version>_amd64.deb` 完整包，另附对应源码和 SHA-256 清单，创建 Release 草稿，检查 CI、产物校验和该版本的实际验证记录后再公开发布。
