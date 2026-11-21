# SkycRemote

远程桌面，设备之间直连。

没有中继服务器。画面、按键、文件全在两台机器之间直接传。
Tracker 只登记地址和在线状态，不碰会话数据。

直连不成的用户用不了。这是项目的原则，不是还没做完的功能。

## 装

打开就是窗口。装好之后，命令行工具 `skycremote` 也在 PATH 上。

| 平台 | 文件 | 怎么装 |
|------|------|--------|
| macOS | `SkycRemote-<版本>.dmg` | 打开 DMG，把 SkycRemote 拖进「应用程序」 |
| Windows | `SkycRemote-Setup-<版本>-x64.exe` | 双击安装程序，跟着走完 |
| Linux | `skycremote-linux-x64.tar.gz` | 解压后运行里面的 `skycremote` |

macOS 上程序第一次启动时，会把命令行工具装进 `/usr/local/bin`；
那个目录写不进去就装到 `~/.local/bin`，并把它加进 shell 的配置里。

Windows 的安装程序会问装给谁：

- **只为我安装**：装在你自己账户下，不需要管理员密码。
- **为所有用户安装**：装进 Program Files，这台机器上的每个账户都能用。
  选这个会弹一次 Windows 的管理员权限提示，那是你自己的系统在问。

命令行工具装好之后就在 PATH 上，两种装法都是。

## 用

两端都要连到同一个 Tracker。Tracker 只登记地址，不转发任何会话内容：

```
skycremote tracker
```

被控端（被操作的那台）：

```
skycremote host --id 被控端的名字 --name 显示用的名字
```

控制端（你坐在前面的那台）：

```
skycremote control --id 被控端的名字
```

`control` 打开的就是窗口。直接双击程序，在窗口里点「共享」或者「控制」，也一样。

## 平台

Windows Linux macOS

## 加密

X25519 密钥交换 HKDF 派生会话密钥 AES-256-GCM 加密信道。
QUIC 的 TLS 1.3 保护传输。Tracker 看不到会话内容。

## 协议

协议版本 1。

控制消息走可靠流，JSON 编码，每条消息前带 4 字节长度。
画面走不可靠数据报，晚到的帧直接丢掉。

## 仓库

crates/skycremote-core 协议、加密握手、打洞、采集、编解码、输入事件
crates/skycremote-tracker 地址登记与在线状态
crates/skycremote-app 窗口和命令行工具
crates/skycremote-installer 安装程序，以及把命令行工具放进 PATH 的那部分

## 许可

MIT
