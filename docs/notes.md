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

这份清单是三轮试出来的，每一轮 GitHub 上只报一行：

| 缺的 | 报的错 |
|---|---|
| GTK / GLib | `Package glib-2.0 was not found` |
| PipeWire | `Package libpipewire-0.3 was not found` |
| GBM | `rust-lld: error: unable to find library -lgbm` |

前两轮是 pkg-config 找不到，最后一轮是链接器找不到 —— 前两个在编译
早期就炸，最后一个要等到链接才炸，所以白等了两轮 CI。

`docker run --rm -v "$PWD":/repo rust:1.90-bookworm` 能在本地把整个
Linux 编译跑一遍，比在 CI 上试快。

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
