# SkycRemote

远程桌面，设备之间直连。

没有中继服务器。画面、按键、文件全在两台机器之间直接传。
Tracker 只登记地址和在线状态，不碰会话数据。

直连不成的用户用不了。这是项目的原则，不是还没做完的功能。

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
crates/skycremote-app 两端程序

## 许可

MIT
