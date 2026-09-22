# AirCard 中文版（AirCard-CN）

Apple 钱包卡面修改工具的中文汉化版，**同时提供 macOS 与 Windows 双平台软件**，可用于修改 iPhone 上 Apple Pay / 钱包中的**银行卡卡面、交通卡卡面**，以及**锁屏密码键盘（拨号键盘）主题**，无需越狱。

> 本项目为整合发行仓库：`macos/` 为 macOS 版（汉化自 [Mak5er/AirCard](https://github.com/Mak5er/AirCard)），`windows/` 为 Windows 版（汉化自 [Lumid-Off/AirCard-Windows](https://github.com/Lumid-Off/AirCard-Windows)）。界面、提示与日志均已全面中文化。

---

## 📥 下载

在右侧 **Releases** 页面下载对应平台的安装包：

| 平台 | 文件 | 说明 |
|---|---|---|
| macOS | `AirCard-macOS-中文版.dmg` | 通用架构（Apple Silicon + Intel），拖入应用程序即可 |
| Windows | `AirCard-Windows-中文版.exe` | 64 位，免安装，直接运行 |

> 每个 Release 同时包含 macOS 与 Windows 两个安装包。

---

## ✨ 功能

- **银行卡 / 交通卡卡面**：替换 Apple Pay 钱包中卡片图案（自动缩放至 1536×969 标准卡面）
- **锁屏密码键盘主题**：应用 Cowabunga / Nugget 的 `.passthm` 主题包，支持自定义语言与粗体
- **卡片哈希扫描**：通过系统日志实时捕获目标卡片，无需手动输入
- **USB / WiFi 双传输**：自动或手动选择连接方式
- **界面全面中文**：标签页、按钮、提示、日志、错误消息均为简体中文

---

## 🚀 快速上手

### 前置条件

1. iPhone 系统 iOS 18+（macOS 版另支持部分旧版本）
2. **Windows**：安装 64 位 iTunes 或 Apple 移动设备支持（[Apple 官网下载](https://www.apple.com.cn/itunes/)）
3. iPhone 通过 USB 连接电脑并解锁，点按 **「信任此电脑」**
4. 可选：启用 WiFi 同步后，可在同一局域网内无线连接

### 修改银行卡 / 交通卡卡面

1. 打开 AirCard，点击顶部「刷新」确保设备已连接
2. 在「钱包」页点击 **扫描**，在 iPhone 上打开 Apple 钱包并轻点要修改的卡片
3. 卡面哈希自动捕获后，选择卡面图片（PNG/JPG/WebP 均可，自动缩放）
4. 点击 **应用卡面**，等待写入完成
5. 在 iPhone 上强制关闭钱包 App（上滑卡片）并重新打开，即可看到新卡面

### 修改锁屏密码键盘主题

1. 切换到「密码键盘」标签页
2. 选择 `.passthm` 主题包（可从 Cowabunga / Nugget 社区获取）
3. 按 iOS 版本选择目标缓存（iOS 18+ 选 TelephonyUI-10，iOS 16-17 选 TelephonyUI-9）
4. 点击 **应用密码键盘主题**，完成后锁定 iPhone 查看新键盘

---

## 🛠 从源码构建

### macOS

```bash
cd macos
chmod +x build.sh
./build.sh
# 产物：macos/build/AirCard.dmg
```

需要 Xcode Command Line Tools（`xcrun clang` / `swiftc`）。

### Windows

```bash
cd windows
cargo build --release
# 产物：windows/target/release/aircard.exe
```

需要 Rust 工具链（[rustup](https://rustup.rs)）与 MSVC 构建工具。

---

## 🤖 自动构建

本仓库配置了 GitHub Actions（`.github/workflows/build.yml`）：推送 `v*` 标签后自动构建 macOS DMG 与 Windows exe，并发布为包含双平台安装包的 Release。

---

## ⚠️ 免责声明

- 本工具利用系统同步机制修改系统文件，仅供**学习研究**用途，请自行评估风险
- 卡面修改后，系统更新或重启可能导致改动失效
- 请勿用于任何商业或违法用途
- 本仓库与 Apple Inc. 无关

## 🙏 致谢

- macOS 版上游：[Mak5er/AirCard](https://github.com/Mak5er/AirCard)
- Windows 版上游：[Lumid-Off/AirCard-Windows](https://github.com/Lumid-Off/AirCard-Windows)
