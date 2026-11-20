# 开发笔记

记的是决定和原因，不是过程流水账。

## 屏幕尺寸的换算放在数据里，不放在 cfg 里

三平台的采集后端报的尺寸单位不一样：macOS 和 Linux 报的是逻辑单位
（点），要乘缩放系数才是像素；Windows 的 `dmPelsWidth` 已经是物理像素，
再乘一次就会把 4K 屏说成 7680 宽。

这个差异一开始写成 `#[cfg(target_os = "windows")]`，结果在 macOS 上跑
Windows 的测试时，测的是 macOS 的分支 —— Windows 的数字根本测不到。
`cfg` 描述的是「代码跑在哪台机器上」，而这里要说的是「这个后端报的是
什么单位」，两件事。

所以改成平台模块各自声明 `SIZE_IS_PIXELS`，`make_screen` 看数据决定
乘不乘。这样在任何机器上都能验各平台的数字。

参数也从八个散装参数收进了 `ScreenSpec`：这八个字段永远一起出现，
它们是「一块屏幕的描述」，不是八个无关的数。

## Linux 上编译要装的东西

`tao`/`wry` 要 GTK 和 WebKitGTK，`xcap` 要 PipeWire，`webkit2gtk` 链接
时还要 Mesa 的 GBM。清单写在 `.github/workflows/ci.yml` 的 `LINUX_DEPS`
里，两个 job 共用一份 —— 同一份清单写两遍，漏的总是没人看的那一份。

这份清单是四轮试出来的，每一轮 GitHub 上只报一行：

| 缺的 | 报的错 |
|---|---|
| GTK / GLib | `Package glib-2.0 was not found` |
| PipeWire | `Package libpipewire-0.3 was not found` |
| GBM | `rust-lld: error: unable to find library -lgbm` |
| libclang | `Unable to find libclang`（bindgen 生成 pipewire 绑定时） |

前两轮是 pkg-config 找不到，第三轮是链接器找不到，第四轮是 bindgen
找不到 —— 一个比一个晚炸，所以一轮白等一次 CI。

**验证的地方**：本机有 OrbStack 的 Linux 机器 `ServerForTest`
（Ubuntu 26.04，arm64，192.168.139.149），`orb -m ServerForTest <命令>`
就能进去跑。比在 CI 上试快得多，也不占 GitHub 的 runner。

代码怎么进去：`orbctl push` 只认 `~/OrbStack/<机器名>/home/<用户>/`
这个路径，而且 macOS 的扩展属性会挡住 `cp`，要加 `-X`：

```
tar czf /tmp/src.tar.gz --exclude=target --exclude=.git <要传的东西>
cp -X /tmp/src.tar.gz ~/OrbStack/ServerForTest/home/skyc8266/
orb -m ServerForTest tar xzf ~/src.tar.gz -C ~/项目目录
```

SSH 直连那台机器不通（`connect to host 192.168.139.149 port 22:
Operation timed out`），走 `orb` 才进得去。

## 无显示器的 Linux 机器上，报错要说人话

在一台没有 X11 也没有 Wayland 的机器上（服务器、容器、SSH 会话），
`list_screens()` 会把 X11 客户端库的一句内部消息原样抛出来：

```
the capture backend failed: Connection closed, error during parsing display string
```

用户看到的是「连接关闭」，不知道该做什么 —— 重试多少次都没用，因为
根本不是可重试的故障。

`linux::explain_error` 现在认得这类错，返回 `Unsupported{platform: "X11"}`
并说清楚「这台机器没有显示服务器可采集，需要登录到机器自己的桌面」。
X11 客户端库有好几种说法（display string 解析失败、屏幕上没有匹配的
screen、XOpenDisplay 失败、I/O 错误），全都认。

原有的 `CaptureError::Backend` 注释写着「这个值得重试」，所以这个错误
放进去本身就是错的 —— 它不是重试能解决的，是环境问题。

这个 bug 是**在 Linux 虚拟机上跑测试才发现的**：macOS 和 Windows 上
永远碰不到，所以 CI 里只有 Linux job 能抓到它。

## workflow 自己也要被检查

一个解析不了的 workflow 不会大声失败：GitHub 直接拒绝那个文件，run
回来零个 job，日志只有一句 `failed to get run log: log not found`，
什么错都没说。

所以 CI 里加了一条 actionlint。几点：

- actionlint 不是 GitHub Action，是个 Go 工具加 pre-commit hook，官方
  仓库里没有 `action.yml`，`uses:` 引用不了
- 它的下载脚本是 bash，Windows runner 里的 bash 跟 Linux 的不是一回事，
  管道进去会挂。所以这一步只在 Linux 上跑 —— 检查的是 workflow 结构，
  跟平台无关
- 本机装了 `actionlint`（`brew install actionlint`），改完 workflow
  本地先跑一遍再推

引用同一个仓库的 workflow 要用相对路径 `./` 开头，写成完整仓库名
GitHub 会拒绝。

## 发版

tag push 触发。`check-version` 先看 tag 上的版本号和三个 crate 写的
版本号一不一致，不一致直接拦住 —— 不然发出去的包报的是错的版本号。

二进制不在发版 workflow 里重新构建，而是调用 CI 那个 workflow：
依赖清单和构建步骤只有一份，测试跑在前面，发出去的包一定是过了测试
的构建。

`release-notes.md` 就是 release 的正文。
