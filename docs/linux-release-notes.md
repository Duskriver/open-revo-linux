# OpenRevo for Linux v0.9.3-linux.1 — 社区预览版

基于 [faintonce/open-revo](https://github.com/faintonce/open-revo) 的社区 Linux 移植，由 [Duskriver](https://github.com/Duskriver) 维护，沿用 GPL-3.0-or-later 许可。

## 本次提供

- Tauri 原生 Linux 桌面窗口、主题切换、诊断导出与登录启动入口。
- 通过 Linux 标准接口读取 CPU、内存、温度、电源与电池状态。
- 在系统支持且授权允许时切换系统电源档位、调节背光和设置电池限充。
- x86_64 Debian 安装包、对应源码归档和 SHA-256 校验文件。

## 安装

下载本页 `.deb`，在文件所在目录运行（将文件名替换为实际下载的名称）：

```sh
sudo apt install ./OpenRevo_0.9.3-linux.1_amd64.deb
```

安装后通过桌面应用启动器以普通用户运行。包由 Ubuntu 22.04 CI 构建，依赖 GTK3、WebKitGTK 4.1 和 Ayatana AppIndicator。其他发行版可按仓库 Linux 文档从源码构建。

## 已知限制

- 这是开发预览版，尚未达到上游 Windows 版的硬件功能完整度。
- EC 风扇曲线、OEM 功耗调节、模式灯、RGB、水冷、显卡 MUX 和 Linux 自动更新仍待移植。
- 系统电源档位不等于 OEM EC 模式；可用操作取决于机型、内核接口和 Polkit 授权。
- 实机验证环境为 GM5HG7A / Ryzen 7 8845H / Ubuntu 26.04.1，未完成所有机型和发行版兼容性测试。
- Windows 编译检查用于验证平台隔离，不能替代 Windows 实机回归测试。

源码和构建步骤见本页源码归档中的 `docs/linux-port.md`；问题反馈请使用 [本 Fork Issues](https://github.com/Duskriver/open-revo-linux/issues)，附上发行版、内核、机型和复现步骤。
