## SkycRemote v0.2.1 - Pre-release

远程桌面，设备之间直连。没有中继服务器，画面和按键全在两台机器之间直接传。
Tracker 只登记地址和在线状态，不碰会话数据。

### 本次更新中修复了下列问题：

- 滚轮快了约一百倍：一个滚轮刻度在三个平台上都被当成一百格执行。协议的滚轮量现在明确按像素传，Windows 和 X11 攒够一格才滚，攒不满的零头留到下一次
- 控制端把浏览器的 `deltaMode` 归一化成像素，Firefox 那种按「行」给量的浏览器不再滚起来像没反应
- macOS 的滚轮改成按像素投递，手感和本机一致

### 感谢

- Skyc8266：把滚轮档位这个量级问题摆到台面上

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
- **滚轮只验到换算关系**。一个刻度折成一格、零头会累积，这两点有测试；实际滚动的速度和手感要在真机上确认。
- 还没有文件传输。

## 许可

[MIT](https://github.com/Mofan2020/SkycRemote/blob/main/LICENSE) © Skyc8266
