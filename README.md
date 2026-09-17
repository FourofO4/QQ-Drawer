# QQ 抽屉 · QQ Drawer

贴在 Windows 10 桌面上的一条 QQ 消息抽屉。收起时是一条透明的窄条，显示「谁：说了什么」；点一下就展开成一个聊天窗口，能看历史、看图、能打字回复。

**电脑上不登录 QQ 客户端**——消息来自本机运行的 NapCat（无头 QQ）+ OneBot 11 WebSocket，登录态留在手机 QQ 上。

> **状态**：v1.0 代码已完成。前端 127 个单测全绿，`tsc --noEmit` 无错，生产构建通过。
> Rust 侧写了 210 个单测，本机环境详见 [§ 环境准备](#1-环境准备)；若尚未跑过 `cargo test`，请以那里为准确认验证状态。
>
> 实现依据：[`docs/开发规格说明书.md`](docs/开发规格说明书.md)（需求、交互、数据模型、踩坑清单全在里面）
> 形态预览：[`preview/qq-drawer-preview.html`](preview/qq-drawer-preview.html)（单文件零依赖，双击打开）

---

## 它长什么样

```
收起态（可拖动、可吸附屏幕顶边）
┌────────────────────────────────┐
│ ▍小陈： 今天几点到？             │   40px 高，无头像、无边框、无未读数字
└────────────────────────────────┘

展开态（就地展开，窗口位置不动）
┌──────────────────────────────────────────────┐
│ 小陈  大前端交流群  产品组 静音  ···   [锁定] ▲ │  顶栏标签式，不是侧边栏
├──────────────────────────────────────────────┤
│                 今天 14:02                    │
│  ⬤ 李工                                       │
│     ┌──────────────────────────┐              │
│     │ @你 那个接口的字段名定了没？│  ← @我高亮   │
│     └──────────────────────────┘              │
│                        ┌────────────┐         │
│                        │ 定了，晚点同步│         │
│                        └────────────┘         │
├──────────────────────────────────────────────┤
│ ┌──────────────────────┐  ┌──────┐            │
│ │ 发消息…               │  │ 发送 │            │
│ └──────────────────────┘  └──────┘            │
└──────────────────────────────────────────────┘
```

## 特性

- **折叠条是字幕式的**，不是通知卡片。无头像、无气泡、无红点数字——头像加胶囊是"通知栏"的语汇，不是桌面组件的语汇。
- **展开就是就地展开**，窗口左上角不动，向右下长出面板。未锁定时点一下桌面别处自动收起，也可以锁定常驻。
- **提醒是整条闪烁 3 次**（约 1.8 秒）后安静下来，只留一道轻微痕迹（字重 + 亮度 + 左侧 2px 竖条）。没有声音、不闪任务栏、不出现在任务栏、也不出现在 `Alt+Tab`。
- **免打扰只在本机生效**。给"小陈"单独静音，手机上 QQ 的设置一点不变——因为技术上就写不进去（OneBot 也没有这个字段）。
- **静音不等于看不见**。静音会话来消息不闪不响，但依然会更新折叠条内容、展开后照常阅读；只是永远抢不到折叠条的显示位置。
- **轻量是硬指标**：常驻内存合计 ≤ 100 MB，空闲 CPU ≤ 0.5%，安装包 ≤ 10 MB。对标的是 NTQQ 客户端，不是同类小工具。

---

## 1. 环境准备

只有三项：Node.js、Rust、一个 C 链接器。

| 组件 | 版本 | 用途 | 本机位置 |
| --- | --- | --- | --- |
| **Node.js** | ≥ 20 | 前端构建与单测 | 系统 PATH 上的 `node` |
| **Rust** `stable` | 1.98.1 | 后端编译 | `R:\Rust\toolchain` |
| **MinGW-w64**（GCC） | 16.2.0 | Rust `windows-gnu` 的链接器；也是编 SQLite 的 C 编译器 | `R:\Rust\mingw64` |

### 1.1 本机已装好的配置

工具链整体在 R 盘，靠用户级环境变量指过去：

```
CARGO_HOME  = R:\Rust\cargo          # cargo 的配置与依赖缓存
RUSTUP_HOME = R:\Rust\rustup         # 仅 rustup 用，本项目实际不依赖它（见 §1.3）
PATH       += R:\Rust\toolchain\bin;R:\Rust\mingw64\bin
```

**这两个 PATH 项必须在最前面。** 系统里还躺着两套旧 MinGW —— `R:\Dev-Cpp\MinGW64` 的 gcc 4.9.2 和 `R:\VScodec\mingw64` 的 MinGW.org 8.2.0，mingw-w64 运行时都太旧。它们一旦抢先被找到，链接会以很难查的方式失败。`R:\Rust\cargo\config.toml` 里另外把 linker 写成了绝对路径，双保险。

验证：

```bash
cargo --version     # cargo 1.98.1
rustc -vV           # rustc 1.98.1 / host: x86_64-pc-windows-gnu / LLVM 22.1.8
gcc --version       # gcc (MinGW-W64 x86_64-ucrt-posix-seh) 16.2.0
```

### 1.2 为什么用 GNU 工具链而不是 MSVC

Tauri 在 Windows 上官方推荐 MSVC，但 **MSVC 需要一个管理员权限的 Visual Studio Build Tools 安装（约 3 GB）**，本机没有装也没有管理员权限，所以选了免安装、解压即用的 `x86_64-pc-windows-gnu` + MinGW-w64。

本项目不依赖任何 MSVC 专有特性，所以 `cargo test` / `cargo build` 正常可用。以后若拿到管理员权限要切回 MSVC，两步即可：

```bash
rustup toolchain install stable-x86_64-pc-windows-msvc
rustup default stable-x86_64-pc-windows-msvc
```

装 VS Build Tools 时勾选「使用 C++ 的桌面开发」。MinGW 可以留着不用。

### 1.3 为什么工具链不是 rustup 装的

`R:\Rust\toolchain` 下的 Rust 是**手工装的**，不是 rustup 装的。

现象：`rustup-init` 跑起来后一动不动，没有进度，也没有报错，放两个小时还是那样。

排查：看它留下的临时目录，它在写 `<RUSTUP_HOME>\tmp\<随机>_dir\components` 时被拒绝——
`os error 5 拒绝访问`。写不进去就回滚重试，重试又失败，于是无限循环，界面表现就是"卡住"。

> 当时把原因记成了"路径名叫 `components` 就被拒"，**那是误判**。后来在 `cargo test` 上
> 撞到同一个错误码，才定位到真正的原因：这台机器上对工作区之外的文件写入会被安全策略
> 拦截（见 §1.4）。名字纯属巧合。

那个 `components` 只是一份记录"已装哪些组件"的文本文件，手工安装根本不需要它。于是改成：
从官方发行清单里取各组件地址，自己下载 `.tar.xz` 并直接解压到最终位置。脚本留在
`R:\Rust\install-toolchain.py`，幂等，可重复跑。

组件与目录映射规则（各包布局不同，所以按"路径里第一个 `bin`/`lib`/`share`/`etc` 就是安装根"来归一）：

```
rustc-<v>-<triple>/rustc/bin/...        -> toolchain/bin/...
rust-std-<v>-<triple>/rust-std-<t>/lib/...-> toolchain/lib/...
rust-mingw-<v>-<triple>/rust-mingw/lib/...-> toolchain/lib/...   （自带 MinGW 链接库）
clippy-preview-<v>-<triple>/.../bin/... -> toolchain/bin/...
```

换机器要重装时：

```bash
# 1) MinGW-w64：从 https://winlibs.com/ 下 x86_64-posix-seh-ucrt 的 zip
#    解压到 R:\Rust\mingw64（免安装，不需要管理员）
# 2) Rust：跑安装脚本（会自己解析版本、下载、解压、跳过已装好的组件）
python R:\Rust\install-toolchain.py
# 3) 手工设好 §1.1 的三个用户级环境变量
```

> 装 rustup 本身也可以，只是**别用它装工具链**，会撞上上面那个坑。

### 1.4 在沙箱 / 受限环境里构建

**只在自己终端里跑 `cargo build` / `cargo test` 的话，这一节可以跳过。**

症状：`cargo test` 慢得离谱——解包依赖大约 4 个包/分钟（正常是每秒几十个），
跑 29 分钟后以 `failed to unpack ... 拒绝访问 (os error 5)` 失败。

原因：受限环境下对**工作区之外**的写入会被拦截，而且**每一次建文件**都要过一次策略检查。
实测同一台机器上建 1000 字节文件：

| 位置 | 每次建文件耗时 |
| --- | --- |
| 系统临时目录 `%TEMP%` | 0.3 ~ 1 ms |
| 工作区 `R:\Code\QQ Drawer` | 356 ms |
| `R:\Rust` | 356 ms |

cargo 解包 + 编译要建十几万个文件，350 ms/文件是跑不完的。**不是 R 盘慢**——用
PowerShell 的 `[System.IO.File]::WriteAllText` 在 R 盘上测是 0.4 ms/文件。R 盘本身没问题。

解法：把 cargo 的 home 和 target 都挪到临时目录（工具链仍在 `R:\Rust\toolchain`，只读不受影响）。

```bash
# 复制 .crate 缓存（84 MB）+ 稀疏索引（33 MB）+ config.toml 到临时目录
python R:\Rust\setup-build-home.py

set CARGO_HOME=%TEMP%\qq-cargo-home
set CARGO_TARGET_DIR=%TEMP%\qq-drawer-target
cd src-tauri
cargo test --offline
```

`R:\Rust\run-cargo-test.py` 把上面这段打成了一个脚本，日志写在 `%TEMP%\qq-cargo-test.log`。

> 脚本里刻意**不做任何删除**：bash 的 `rm` 走的是回收站垫片，在 R 盘上回收站调用会失败并
> 按 fail-closed 卡住（实测卡死 3 分钟零输出）。日志文件用 `wb` 截断代替删除。

---

## 2. 准备 NapCat（一次性，约 10 分钟）

1. 获取 NapCatQQ（Windows 版），按官方文档完成首次登录（手机 QQ 扫码授权）。
2. 打开 WebUI（默认 `http://127.0.0.1:6099`）→ **网络配置 → 新建 → WebSocket 服务器**：
   - `host`：`127.0.0.1`（**不要**监听 `0.0.0.0`，不要映射到公网）
   - `port`：`3001`
   - `token`：自己设一个长字符串（**必设**，该端口默认裸奔）
   - 打开 **"上报自身消息"**（`reportSelfMessage`）——这是收到 `message_sent.*` 事件的前提，否则你在抽屉里发出的消息拿不到真实 `message_id`。
3. **保持手机 QQ 在线**。NapCat 在线时电脑端 QQ 客户端会被互踢，这正是我们要的效果。

## 3. 运行

```bash
npm install

# 只调界面：浏览器里跑，用的是内置假后端，不需要 NapCat
npm run dev            # → http://localhost:5173

# 完整应用：Tauri 窗口 + Rust 后端，需要 NapCat 已就绪
npm run tauri dev
```

首次连上后：托盘图标右键 → 设置 → 连接 → 填 `ws://127.0.0.1:3001` 与 token。连上后折叠条出现，点击展开。

## 4. 构建安装包

```bash
npm run tauri build -- --bundles nsis
```

只出 NSIS 安装包，产物在 `src-tauri/target/release/bundle/nsis/`。目标机需要 WebView2 Runtime（Win10 较新版本已内置，安装包用 bootstrapper 模式兜底）。

---

## 开发

### 常用命令

| 命令 | 作用 |
| --- | --- |
| `npm run dev` | Vite 开发服务器（走 mock 后端，脱离 NapCat 调界面） |
| `npm test` | 前端单测（vitest，127 个用例） |
| `npm run test:watch` | 单测 watch 模式 |
| `npm run build` | `tsc --noEmit` + 生产构建 |
| `npm run tauri dev` | 起完整应用（Rust + WebView2 窗口） |
| `cargo test` | Rust 单测，在 `src-tauri/` 下执行 |
| `cargo clippy -- -D warnings` | 静态检查（规格 §5 要求零警告） |
| `cargo fmt` | 格式化 |

### 目录结构

```
qq-drawer/
├── src/                              # 前端
│   ├── core/                         # 纯逻辑，无副作用，最容易被单测钉住
│   ├── ui/                           # 折叠条 / 面板 / 消息列表 / 输入框 / 浮层
│   ├── state/
│   │   ├── ipc.ts                    # 唯一允许出现 Tauri API 的文件
│   │   ├── store.ts                  # 前端视图状态
│   │   └── events.ts                 # Tauri 事件订阅
│   ├── dev/mock.ts                   # 浏览器里用的假后端（生产构建会被摇掉）
│   └── styles/
├── src-tauri/src/                    # Rust 后端
│   ├── ob/                           # OneBot 协议隔离层（事件名/字段/消息段只出现在这里）
│   ├── store/                        # SQLite 访问层（唯一有写权限的地方）
│   ├── sched/notify.rs               # 提醒优先级（纯函数）
│   ├── media.rs                      # 图片落盘 / 去重 / LRU / media:// 协议
│   ├── window.rs tray.rs hotkey.rs   # 无边框窗口几何、托盘、全局快捷键
│   ├── cmd.rs                        # 暴露给前端的 IPC 命令
│   └── lib.rs                        # 装配
├── tests/                            # 前端集成测试
├── docs/                             # 规格说明书
└── preview/                          # 交互形态预览（静态 HTML）
```

### 架构上不能破的四条

1. **单一数据源**——只有 Rust 侧的 SQLite 是权威数据。前端不缓存业务状态，只渲染 Rust 推来的快照与增量。
2. **协议隔离**——OneBot 的一切（事件名、字段、消息段）只准出现在 `src-tauri/src/ob/` 里，上层只认 `ob::model` 的规范模型。QQ 协议改版时只动这一层。
3. **窗口与内容解耦**——窗口的尺寸/位置/模糊效果由 Rust 掌握，前端只画内容，**永远不直接改窗口几何**。前端唯一改窗口的动作是「请 Rust 展开」这一个命令。
4. **动画不上窗口**——一切视觉动效在 CSS 层（只做 `opacity` / `transform` / `background-color`）。逐帧改窗口尺寸会让 DWM 每帧重算背景模糊，必卡。

### 测试约定

- **纯逻辑必须有单测**：提醒优先级、消息段解析、墓碑过滤、幂等、退避序列、快捷键规范化——这些全是无副作用函数，是最容易出错也最容易测的部分。
- 前端集成测试（`tests/pipeline.test.ts`）打桩 Tauri 之后走真实的 `ipc` / `store` / `core`，把「收到消息 → 折叠条显示什么」整条链路钉住。
- 界面本身走人工走查（规格 §5 的约定）。
- **每次改动都要保证测试全绿**，见 [`AGENTS.md`](AGENTS.md)。

---

## 技术栈

| 层 | 选择 |
| --- | --- |
| 应用框架 | Tauri 2（安装包 4–8 MB，常驻远低于 Electron） |
| 渲染 | WebView2 + TypeScript + Vite + Solid.js |
| 数据与逻辑 | Rust（OneBot 适配层、SQLite 持久层、提醒决策） |
| 存储 | SQLite（WAL 模式）+ 本地图片缓存 |
| 传输 | OneBot 11 over WebSocket |

数据落点（全部在本机，无遥测）：

| 项 | 位置 |
| --- | --- |
| 数据库 | `%LOCALAPPDATA%\qq-drawer\data.db` |
| 图片 | `%LOCALAPPDATA%\qq-drawer\media\<sha256[0:2]>\<sha256>.<ext>` |
| 日志 | `%LOCALAPPDATA%\qq-drawer\logs\qq-drawer-YYYYMMDD.log`（环形，保留 7 天，**不记消息正文**） |

## 已知限制

1. **读不到也写不了 QQ 自带的免打扰设置**。OneBot 事件里没有这个字段，NapCat 也没有对应接口。所以本地静音是唯一的办法——好处是不可能误改你手机上的设置。
2. **NapCat 不保存历史**。消息 ID 与文件标识受 LRU 管理，约 5000 条后清理，撤回的消息无法回查。所以图片必须收到即落盘，本地缓存是唯一的副本——**清理 = 永久丢失**。
3. **掉线期间的事件永久丢失**，包括撤回。掉线时发生的撤回，本地会一直显示为正常消息。
4. **Win10 没有原生圆角**，圆角只能在 CSS 层画；模糊用 `apply_blur` 而不是 `apply_acrylic`（后者在 Win10 拖动/缩放时明显掉帧）。
5. **不嵌入壁纸层**。只有"始终置顶 / 不置顶"两档，不置顶时会被其它窗口盖住。
6. **NapCat 在线时电脑端官方 QQ 会被互踢**（符合设计预期；手机端可同时在线）。

## 免责声明

本项目为个人自用的第三方工具，与腾讯公司无关，未获得任何形式的官方授权或认可。

不修改、不破解、不注入 QQ 客户端本体，只把第三方协议端（NapCat）暴露的标准 OneBot 接口渲染成一个桌面组件。使用第三方协议端**存在账号被限制登录的风险**，请在了解并接受该风险的前提下自行决定是否使用；不要用于商业用途或任何自动化群发场景。不分发、不销售、不提供在线服务。
