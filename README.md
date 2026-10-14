# 项目里程碑指挥台 — Windows 桌面版

把现有的单文件网页版移植为 Win10 绿色版：免安装、自带运行时、数据跟着程序走。

## 为什么是这个架构

渲染内核仍然是 Chromium（系统的 WebView2），所以网页版那些渲染优化——
行级 `content-visibility`、共享底纹层、依赖连线按模型算坐标——**原样继承，不用重做**。
Rust 宿主只做三件事：开窗口、提供本地存储、提供原生保存对话框。

| | 安装包 | 常驻内存 |
|---|---|---|
| 本方案（Tauri + WebView2） | ~135 MB（含离线运行时） | 宿主 5–10 MB + WebView2 80–150 MB |
| Electron | 150 MB+ | 250–400 MB |

主程序 exe 本身只有几 MB，135 MB 几乎全是打包进去的 WebView2 运行时——
换成联网安装运行时的话整包 3–6 MB，但内网机器装不上，所以选了离线。

## 目录

```
core/              数据层：原子写、备份轮转、路径回落。不依赖 GUI，任何平台可测
src-tauri/         Tauri 宿主：窗口、IPC 命令、打包配置
dist/
  desktop-bridge.js  window.storage → Rust 的桥接
  index.html         由 build-dist.mjs 生成，不要手改
build-dist.mjs     从网页版派生桌面版前端
```

**网页版 `pm-dashboard-v2.html` 是唯一源头。** 桌面版不维护第二份副本：
`build-dist.mjs` 每次从它重新生成，所以网页版改了什么，桌面版重跑一次就同步。
脚本里 5 处转换全部带断言，网页版结构一旦变化会**构建失败并指出是哪一处**，
而不是悄悄产出一个坏掉的程序。

5 处转换：
1. 去掉 Google Fonts 请求，改用系统字体（内网机器无需联网，首屏更快）
2. 注入存储桥接层
3. 桌面窗口观感（禁右键菜单、禁 F5、正文不可选但输入框可选）
4. 状态栏显示真实数据目录
5. Excel / 备份导出改走原生保存对话框

## 出包

### GitHub Actions（推荐）

推一个 `v*` tag，或在 Actions 页面手动触发。产物是
`项目里程碑指挥台-绿色版-win64.zip`。

CI 做了三件值得说的事：
- 数据层单元测试跑在 Linux 上，几十秒就能挡住会毁数据的改动，不用等 Windows runner
- 下载并打包固定版本 WebView2
- **冒烟测试**：真的把 exe 启动起来，确认进程没立即退出、WebView2 子进程被拉起、
  数据目录里确实写出了文件。一个"能编译但一启动就崩"的包不会被发布出去。

换 WebView2 版本改 workflow 顶部的 `WEBVIEW2_VERSION` 即可。

### 本机构建

需要 [VS Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
（勾选「使用 C++ 的桌面开发」）、[Rust](https://rustup.rs/)、Node 18+。

```powershell
npm run dist                       # 生成 dist/index.html
cd src-tauri; cargo build --release
```

调试时 `npm run dev` 即可，开发机上用系统已装的 WebView2，不必准备固定版本运行时。

## 数据

便携优先：数据在 exe 同目录的 `data\`，一个 key 一个 JSON 文件。

- **原子写**：先写临时文件并 `sync_all` 刷盘，再 rename 覆盖。断电或崩溃可能丢掉
  *这一次*的改动，但绝不会留下写了一半的文件——对放在 U 盘上的程序，这是会真实发生的故障。
- **自动备份**：每次覆盖前把旧版本留到 `data\backups\`，保留最近 10 个。
- **读不到的 key 返回空，而不是报错**。网页版那套 storage 在读一个从没写过的 key 时会抛异常，
  为此页面里有一整段"先列目录再读"的兜底逻辑；桌面版不会抛，那条路径自然不会触发。
- **目录不可写时回落到 `%APPDATA%`**，并在界面顶部明确显示实际位置——
  静默把数据写到用户找不到的地方是最糟的结果。

## 测试

```bash
npm run test:core     # 数据层 16 个用例
npm run dist && node ../scratchpad/desktop.js   # 桌面版端到端
```

端到端用例模拟 Tauri 的 IPC（行为与 Rust 端一致：读不到返回 null、不抛异常），
覆盖：首次启动播种、新建项目落盘、**关掉重开后数据还在**、三个页签渲染、
Excel 与备份导出走原生对话框并产出有效文件、程序目录不可写时的警告。

## 已知边界

- **Win10 1803 及以上**（WebView2 的要求）。
- exe 没有数字签名，SmartScreen 首次运行会拦一下，选「仍要运行」。
  要去掉这个提示需要代码签名证书。
- 固定版本 WebView2 不随 Windows 更新打补丁，安全更新要靠改
  `WEBVIEW2_VERSION` 重新出包。这是换取"内网可用"的代价。
