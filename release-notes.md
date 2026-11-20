# SkycRemote v0.1.0

远程桌面，设备之间直连。

没有中继服务器。画面、按键、文件全在两台机器之间直接传。
Tracker 只登记地址和在线状态，不碰会话数据。

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
- 键鼠输入：可打印字符按 Unicode 发，非字符键走系统自己的键码
- 多屏：能看到所有屏幕，可以切换
- 加密：X25519 密钥交换，HKDF 派生会话密钥，AES-256-GCM 加密信道
- 界面：中英双语，切换立刻生效

## 限制

- **直连不成的用户用不了**。两端都在对称型 NAT 后面时打洞不成，程序会直接报错，不会退回中继。这是项目的原则。
- 输入注入目前只实现了 macOS。Windows 和 Linux 能看画面，键鼠还动不了。
- macOS 上要操作别的窗口，程序需要「辅助功能」权限。没给权限时画面照常共享，鼠标键盘不动，程序会提示。
- 还没有文件传输。

## 许可

[MIT](https://github.com/Mofan2020/SkycRemote/blob/main/LICENSE) © Skyc8266
