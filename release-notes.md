## SkycRemote v0.2.0 - Pre-release

远程桌面，设备之间直连。没有中继服务器，画面和按键全在两台机器之间直接传。
Tracker 只登记地址和在线状态，不碰会话数据。

### 本次更新中做出了下列优化：

- 输入注入的提示改成各平台自己说：macOS 讲「辅助功能」权限，Linux 讲 X11 会话和 XTEST，Windows 讲完整性级别，不再是一句笼统的话
- 被控端启动时统一问一次「这台机器能不能注入输入」，不再把 macOS 的判断写死在主流程里

### 本次更新中添加了下列功能：

- Linux 的键鼠注入（X11 / XTEST），借用空闲键位打非 ASCII 字符
- Windows 的键鼠注入（SendInput），可打印字符走 `KEYEVENTF_UNICODE`，别的走虚拟键码
- 滚轮方向按三个平台各自的约定换算：Windows 正值向上，X11 是 Button 4/5，macOS 走 `CGEventCreateScrollWheelEvent`

### 本次更新中修复了下列问题：

- Linux 上打不出中文：键位改完立刻还原，客户端处理事件时查到的是空键符，事件到了但一个字也不出来。改成会话结束时才还原
- Linux 上「X 服务器拒绝改键盘」的误报：`XChangeKeyboardMapping` 在 Xvfb 上返回 0 却真的改了，按返回值判断会把能打字的机器判成不能打字。改成写完读回来核对
- 两条读源码的守卫测试在 Windows runner 上 panic：按 `\n}\n` 找代码块边界，CRLF 检出后找不到

### 感谢

- Skyc8266：提出现有实现「只有 macOS 能操作」，并给出用虚拟机做 Linux 真机验证的路线

---

## 下载

| 文件 | 平台 |
|------|------|
| `skycremote-linux-x64.tar.gz` | Linux 64 位 |
| `skycremote-macos-arm64.tar.gz` | macOS Apple Silicon |
| `skycremote-windows-x64.zip` | Windows 64 位 |

解压后直接运行 `skycremote`（Windows 是 `skycremote.exe`）。
Linux 需要系统里有 `libwebkit2gtk-4.1` 和 `libgtk-3`。

## 怎么用

两台机器都要有 `skycremote`。一台是被控端，一台是控制端。

**被控端**（被操作的那台）：

```
skycremote host --id 被控端的名字 --name 显示用的名字
```

**控制端**（你坐在前面的那台）：

```
skycremote control --id 被控端的名字
```

两端都要连到同一个 Tracker。Tracker 是登记地址的服务，不转发任何会话内容：

```
skycremote tracker
```

默认 Tracker 在 `127.0.0.1:21115`，直连端口 `21116`。

**画面**（控制端）：

```
skycremote ui
```

## 这个版本有什么

- 屏幕采集、缩放、编码（H.264），画面走 QUIC 数据报，晚到的帧直接丢掉
- 键鼠输入：macOS / Linux / Windows 三个平台都实现了。可打印字符按 Unicode 发，非字符键走系统自己的键码
- 多屏：能看到所有屏幕，可以切换
- 加密：X25519 密钥交换，HKDF 派生会话密钥，AES-256-GCM 加密信道
- 界面：中英双语，切换立刻生效

## 限制

- **直连不成的用户用不了**。两端都在对称型 NAT 后面时打洞不成，程序会直接报错，不会退回中继。这是项目的原则。
- **Windows 的键鼠注入没有在真机上验证过**。能编译、键码映射有单元测试，但「按下键之后真有反应」这台机器上验不了。
- **Linux 的键鼠注入只在虚拟 X 服务器（Xvfb）上验过**。真桌面（GNOME / KDE）还没跑过；Wayland 会话不支持，程序会明确报错。
- **macOS 的键鼠注入没有验证过**。当前环境没有「辅助功能」权限，`AXIsProcessTrusted()` 返回 false，注入会被系统静默丢弃。
- 还没有文件传输。

## 许可

[MIT](https://github.com/Mofan2020/SkycRemote/blob/main/LICENSE) © Skyc8266
