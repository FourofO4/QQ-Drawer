# QQ 抽屉 · QQ Drawer

贴在 Windows 10 桌面上的一条 QQ 消息抽屉。收起时是一条透明的窄条，显示「谁：说了什么」；点一下就展开成一个聊天窗口，能看历史、看图、能打字回复。

**电脑上不登录 QQ 客户端**——消息来自本机运行的 NapCat（无头 QQ）+ OneBot 11 WebSocket，登录态留在手机 QQ 上。

> **状态**：v1.0 代码已完成，并已在本机 **真实 QQ 登录的 NapCat** 上跑通（收消息、拉会话、
> 图片落盘、折叠条显示）。前端 173 个单测全绿，`tsc --noEmit` 无错，生产构建通过；
> Rust 侧 255 个单测全绿（`cargo test`，Windows + GNU 工具链）。第一次在本机跑 `cargo test`
> 若报 `0xc0000139`，看 [§1.5](#15-cargo-test-与-windows-应用清单)。
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
- **收起条和面板的透明度分开调**。折叠条常驻桌面、面板只在需要时出现，两者对"透多少"的诉求本来就不一样，所以是设置页里两个独立的滑杆（各自三档预设：清晰 90 / 半透 72 / 通透 45），互不联动；两层都走同一套可读性补偿——越透就把底色压得越深，而不是单纯变透明。
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

### 1.5 `cargo test` 与 Windows 应用清单

如果 `cargo test` 报成这样，问题不在你的代码：

```
error: test failed, to rerun pass `--lib`
Caused by:
  process didn't exit successfully: ...\qq_drawer_lib-<hash>.exe
  (exit code: 0xc0000139, STATUS_ENTRYPOINT_NOT_FOUND)
```

`0xC0000139` 是**加载期**错误，测试进程连 `main` 都没进。原因在清单：

- `tauri-runtime-wry` 的 Windows 对话框实现调用了 `TaskDialogIndirect`，
  这个函数**只有 Common-Controls v6** 才导出，System32 里的 legacy `comctl32.dll`
  （5.82）没有。要拿到 v6，可执行文件必须嵌入一份声明该依赖的清单。
- `tauri-build` 只把清单链接给 `[[bin]]`（发的是 `cargo:rustc-link-arg-bins=`），
  `cargo test --lib` 的测试宿主拿不到，于是被绑到 5.82，加载直接失败。
- 换用更窄的 `cargo:rustc-link-arg-tests=` 也不管用：cargo 里它判的是
  `target.is_test()`，只认 `tests/` 目录下的集成测试；没有 `tests/` 目录时
  cargo 还会直接以 "does not have a test target" 中止构建（上游未修，cargo#10937）。
- 唯一能覆盖单元测试的是不限定的 `cargo:rustc-link-arg=`（cargo 里对应
  `LinkArgTarget::All`，判断是恒定 `true`），但它同时也会作用于 `[[bin]]`。

所以 `src-tauri/build.rs` 里做了两件事：先用
`tauri_build::WindowsAttributes::new_without_app_manifest()` 关掉 `tauri-build` 自带的那份，
再自己生成同一份清单并用 `compile_for_everything()` 发给所有目标——每个二进制里仍然只有一份清单。
`[[bin]]` 的图标和版本信息不受影响，仍由 `tauri-build` 提供。

**GNU 工具链下这一步需要 PATH 里有 `windres` 和 `ar`**（`tauri-build` 编图标时本来就需要，
见 §1.1）。踩过一次就别再把 `new_without_app_manifest()` 删掉了。

---

## 2. 准备 NapCat（一次性，约 10 分钟）

NapCat 是"无头 QQ"本体：它自己登录 QQ，再把标准 OneBot 11 接口开在一个本地端口上。
抽屉只认这个端口，**不碰、不注入 QQ 客户端**。

### 2.1 装与登录

1. 取 NapCatQQ 的 **Windows Node 版**（`NapCat.Shell.Windows.Node.zip`），解压到任意目录（本机是 `R:\NapCat`）。
   zip 里自带 `node.exe`，不需要另装 Node。双击 `napcat.bat` 启动。
2. 浏览器打开 WebUI（默认 `http://127.0.0.1:6099/webui`，带 token 的完整地址启动日志里会打），
   用**手机 QQ 扫码**完成首次登录。
3. **保持手机 QQ 在线**。NapCat 在线时电脑端 QQ 客户端会被互踢，这正是我们要的效果。

> **换 QQ 号登录会清空本地记录。** 抽屉是**单账号**的：`conversation` / `message` 这些表
> 不带账号列，所以一旦发现这次登录的号跟上次不一样，就把上一个号的会话、消息、图片整体
> 清掉，再按新号重新拉取。**你的设置不受影响**——连接地址、token、窗口位置、折叠条宽度
> 都不用重填。细节见文末「已知限制」第 9 条。

> 启动就报 `Error: The specified module could not be found …wrapper.node`（winerror 126）？
> 不是你装错了——官方包本身缺文件，见 [§5.1](#51-napcat-启动即崩winerror-126)。

### 2.2 开一个 WebSocket 服务器

WebUI → **网络配置 → 新建 → WebSocket 服务器**：

| 字段 | 值 | 为什么 |
| --- | --- | --- |
| `host` | `127.0.0.1` | **不要**监听 `0.0.0.0`，不要映射到公网 |
| `port` | `3001` | 与设置里的 `ws_url` 对应 |
| `token` | 一段长随机串 | **必设**。该端口默认裸奔，而 localhost 的 WebSocket 不受同源策略保护，本机任何网页都能连上来 |
| 上报自身消息 | 打开（`reportSelfMessage`） | 收到 `message_sent.*` 事件的前提；不开的话，你在抽屉里发出的消息拿不到真实 `message_id` |
| 消息格式 | `array` | 抽屉按消息段数组解析 |

配置按**登录成功的真实 uin** 命名落盘，即 `R:\NapCat\napcat\config\onebot11_<uin>.json`。

> ⚠️ **别在登录前手写配置文件**：NapCat 会按 WebUI 里列出的"快速登录候选"之外的**真号**另建一份，
> 你预置的那份 uin 对不上就永远不会被读取。现象很有迷惑性——端口不开，日志里连
> `websocketServers` 相关的行都没有。正确顺序是：先扫码登录，再改配置。

### 2.3 改配置不必重启（WebUI 有 REST API）

`POST /api/OB11Config/SetConfig` 在后端走的是 `save() → reloadNetwork()`，会关掉已删除的适配器、
注册并打开新增的，**立刻生效且不丢登录态**（重启则要重新扫码）。鉴权是 JWT，不是裸 token：

```
hash = SHA256(webuiToken + ".napcat")        # hex
POST /api/auth/login  {"hash": "<hex>"}      # → data.Credential（base64 JWT）
后续请求：Authorization: Bearer <Credential>
```

`websocketServers` 的字段（v4.18.28 实测，写错字段名会让**整份配置失效**）：

```js
{ name, enable, host, port, messagePostFormat, reportSelfMessage,
  token, enableForcePushEvent, debug, heartInterval }
// 默认 enable=false / host="127.0.0.1" / port=3001 / messagePostFormat="array"
//      reportSelfMessage=false / token="" / heartInterval=30000
```

注意**没有** `reconnectInterval` / `verifyCertificate`——那些属于 `websocketClients`（反向 WS 客户端）。

### 2.4 把 token 填进抽屉

托盘图标右键 → 设置 → 连接 → `ws://127.0.0.1:3001` + 上面那个 token。

> token 空着会连出一个很有辨识度的假象：**握手是成功的（101），但 NapCat 不回任何动作响应**，
> 日志里 `获取登录信息失败 … 超时（8000 ms）`、`拉取会话列表失败 … 超时（20000 ms）`，
> 几十秒后对端主动关闭。看到这三行就直接去核 token，别怀疑网络。详见 [§5.2](#52-连上了但所有请求都超时)。

## 3. 运行

```bash
npm install

# 只调界面：浏览器里跑，用的是内置假后端，不需要 NapCat
npm run dev            # → http://localhost:5173
```

完整应用（Tauri 窗口 + Rust 后端，需要 NapCat 已就绪）要在 **PowerShell** 里跑：

```powershell
# R 盘 Rust 工具链 + 无空格的 target 目录
# 本仓库路径 R:\Code\QQ Drawer 含空格，CARGO_TARGET_DIR 不设会炸（见 §5.7）
$env:PATH = "R:\Rust\toolchain\bin;R:\Rust\mingw64\bin;" + $env:PATH
$env:CARGO_TARGET_DIR = "$env:TEMP\qq-drawer-target"

npm run tauri dev
```

懒得每次敲这几行，直接跑封装脚本（它把 `PATH` / `CARGO_HOME` / `CARGO_TARGET_DIR`
和首次 `npm install` 都处理了）：

```powershell
.\scripts\dev.ps1
```

**必须在普通终端启动**（PowerShell / Windows Terminal / CMD），不要从 WorkBuddy
或其它沙箱 shell 里起。沙箱会拒绝 `%LOCALAPPDATA%\qq-drawer\media\**` 与
`EBWebView` 临时缓存的写入，导致图片落盘偶发失败（日志里出现 `图片落盘失败`），
详情见 [§5.6](#56-图片落盘失败日志有-图片落盘失败)。

连上之后折叠条出现在屏幕右上角，点击展开。首次要填 token，见 [§2.4](#24-把-token-填进抽屉)。

> **退出只有一条路**：托盘图标右键。窗口是 `skipTaskbar: true` + `closable: false`，
> 任务栏里看不到、`Alt+Tab` 里也没有。
> 另有两条全局快捷键：`Ctrl+Alt+Q` 展开/收起，`Ctrl+Alt+M` 静音切换
> （`Ctrl+Alt+M` 可能被别的程序占用，日志里会打 `HotKey already registered`，属正常降级）。

## 4. 打包（出可双击运行的程序）

本机已经有一份打好的，直接双击就能用：

```
R:\QQ-Drawer\QQ-Drawer.exe
```

要重新打，PowerShell 里跑（`CARGO_TARGET_DIR` 的要求同 §5.7，不设会在链接期炸）：

```powershell
$env:PATH = "R:\Rust\toolchain\bin;R:\Rust\mingw64\bin;" + $env:PATH
$env:CARGO_TARGET_DIR = "$env:TEMP\qq-drawer-target"

npm run tauri build -- --bundles nsis
```

或者直接用封装脚本，它跑完会把产物归拢到 `R:\QQ-Drawer\`：

```powershell
.\scripts\build.ps1
```

产物：

| 文件 | 说明 |
| --- | --- |
| `R:\QQ-Drawer\QQ-Drawer.exe` | 便携版，双击直接跑（需要 WebView2 Runtime，Win10 较新版本已内置） |
| `R:\QQ-Drawer\QQ-Drawer-Setup.exe` | NSIS 安装包（跑 `build.ps1` 才生成）。**当前用户**安装，开始菜单里有快捷方式 |

**release 构建很慢**（fat LTO + `opt-level="z"`，首次约 25~30 分钟，且 MinGW ld 链接是瓶颈）。
`%TEMP%\qq-drawer-target` 里的依赖是缓存复用的，别随手删。

### 4.1 日常启动：双击一个文件就够了

**是的——NapCat 必须先跑起来，它就是那个"服务器"。**

抽屉本身只是个客户端，所有 QQ 消息都从 NapCat 的
`ws://127.0.0.1:3001` 拿。NapCat 不在，抽屉会一直重连（日志里刷
`连接中断 error=连接失败 … attempt=0/1/2…`），界面上什么也没有。

**但顺序其实不挑：** 抽屉的重连是指数退避的（0.5s → 1s → 2s → 4s → 8s → 16s …），
所以你先点抽屉、再起 NapCat 也行，NapCat 上线后几秒内抽屉会自己接上。

懒得管顺序就用这个（`scripts/start.bat` 的副本，放在 exe 旁边）：

```
R:\QQ-Drawer\Start QQ Drawer.bat
```

双击它，它会：

1. 用 `netstat` 查 `3001` 有没有在监听，**没在听才**起 NapCat
   （工作目录 `R:\NapCat`，命令 `node.exe index.js`，等价于 `R:\NapCat\napcat.bat`）。
   NapCat 约 10~30 秒启动完；**如果 QQ 登录态过期，它那个窗口会打出二维码**，
   用手机 QQ 扫一下。
2. 用 `tasklist` 查 `QQ-Drawer.exe` 在不在，在就跳过，不在才启动它；
   剩下的交给它的自动重连。

**两步都是幂等的，连点几次没关系**——不会堆出第二个 NapCat，也不会堆出第二个抽屉。

> 这个应用本身**没有单实例保护**（全局搜 `single_instance` 无结果），直接双击两次
> `QQ-Drawer.exe` 会在托盘里堆两个图标、屏幕上叠两条折叠条。所以才要在脚本里拦一道。

脚本是纯 ASCII + CRLF（cmd.exe 按 OEM 码页读 `.bat`，中文会变乱码甚至让
`goto :label` 解析失败），路径写死在文件顶部，NapCat 或 exe 搬家了改那两行就行。

**目前没有配任何开机自启**（`HKCU\...\Run` 里只有 ctfmon / OneDrive / IDM / Docker / Edge）。
要开机自动跑，手动做一个：`Win+R` → `shell:startup` → 把
`R:\QQ-Drawer\Start QQ Drawer.bat` 的快捷方式丢进去。

> 想给 exe 单独放桌面：右键 `QQ-Drawer.exe` → **发送到 → 桌面快捷方式**。
> 单独双击它只开抽屉，不拉 NapCat——NapCat 得另外起。
>
> **配置不用重填。** 抽屉连的还是 `%LOCALAPPDATA%\qq-drawer` 里那份，dev 阶段
> 填好的 `ws_url` / token 会自动沿用。**换机器才需要重填**，见 [§2.4](#24-把-token-填进抽屉)。

### 4.2 仓库里只放源码，不放二进制

上面这两个 exe 只存在于本机 `R:\QQ-Drawer\`，**不进版本库**：

- 它们是构建产物，`scripts\build.ps1` 随时能重新产出，进 git 历史没有意义；
- 便携版 6.5 MB、NSIS 安装包更大，进了历史就是每次 `git clone` 都得拖着走。

`.gitignore` 已经把该拦的都拦了：`release/`、`dist/`、`src-tauri/target/`，以及根目录
下散落的 `/QQ-Drawer.exe`、`/QQ-Drawer-Setup.exe`、`/WebView2Loader.dll`、`*.pdb`。
**产物一律归拢到 `R:\QQ-Drawer\`，别在仓库根目录留副本**——留了也提交不上去，
只会在别处多一份容易过期的旧文件。

> 历史里曾经进过一份 `QQ-Drawer.exe`（提交"打包成可执行文件"，6.5 MB）。
> 2026-09-20 用 `git filter-branch --index-filter` 把它从整条历史里摘掉了，
> `.git` 从 4.3 MB 降回 441 KB，那个提交本身因为变空也被 `--prune-empty` 丢弃。
> 注意这类操作**改写的是历史**：远端必须强推（`git push --force-with-lease`），
> 别处已经克隆过的副本要删掉重新 clone，否则会一直指着已失效的旧提交。

---

## 5. 排障

按"症状"查。这几条都是本机真实撞过的，不是想象出来的。

> **九成的"连不上"其实是 NapCat 没起。** 日志里刷
> `连接中断 error=连接失败 ws://127.0.0.1:3001/?access_token=… attempt=0/1/2/3…`
> 并且间隔按 0.5s → 1s → 2s → 4s → 8s → 16s 拉长，就是这个。
> 先 `netstat -ano | findstr ":3001" | findstr "LISTENING"` 看一眼，
> 没输出就去起 NapCat，或者直接双击 §4.1 那个脚本。
> 注意这条**不是** §5.2 —— §5.2 是"握手成功但没人应答"，症状差在日志里有
> `WebSocket 已连接` 那一行。

### 5.1 NapCat 启动即崩（winerror 126）

```
Error: The specified module could not be found. …\wrapper.node
```

**是官方包自己缺文件，不是你解压坏了。** 解析 `wrapper.node` 的 PE 导入表可以看到
33 个**静态**导入里缺 `crypto.dll` 和 `ssl.dll`（不是 delay-load，没法靠"用到才加载"绕过）。
NapCat 的 CI 打包任务只从 QQ 安装包里抠 12 个文件，漏了这两个。

补法：从 QQ 官方安装包（`QQ_9.9.31_260528_x64_01.exe`，CI 里用的同一个 sha256）里
解出 `resources\app\crypto.dll` 与 `ssl.dll`，复制到 NapCat 目录即可。
这个 exe 本身就是 7z SFX，用 NapCat 自带的 `7z.exe` 直接展开。

> 判别手法：**先把静态导入表和延迟导入表分开**（延迟描述符 `ImgDelayDescr` 是 32 字节，
> 不是静态的 20 字节，写错会解析出乱码把结论带偏）。`api-ms-*` / `ext-ms-*` 报 not found
> 属正常噪声，那是 API set 虚名，由加载器解析。

### 5.2 连上了，但所有请求都超时

日志长这样：

```
INFO  WebSocket 已连接 url="ws://127.0.0.1:3001"
WARN  获取登录信息失败 error=get_login_info 超时（8000 ms）     ← 精确 8 秒
WARN  拉取会话列表失败 error=get_recent_contact 超时（20000 ms） ← 精确 20 秒
WARN  连接中断 error=对端发来关闭帧
```

两个完全不同的原因会给出**同一种**日志，先分开：

- **`url` 里没有 `?access_token=`** → 设置里 token 是空的。NapCat 让握手过（101），
  但不认这个连接，于是不回任何动作响应，几十秒后关闭。用 §5.4 的探针带上 token 一试就能确认。
- **`url` 里带了 token，仍然超时** → 这是代码缺陷，已在 `b57fa54` 修掉：
  读循环启动晚于初始化请求，而响应的派发就在读循环里，于是初始化那两条请求根本没人接。
  特征是 `self_id` 停在 0、启动时拉不到会话列表，属**静默降级**。升级到该提交之后的版本即可。

### 5.3 明明配好了，端口就是不开

先去 NapCat 日志里搜有没有 `WebSocket服务: 127.0.0.1:3001 … 已启动`。

- **一个字都没有** → 你改的那份 `onebot11_<uin>.json` 的 uin 不是当前登录的号。
  NapCat 按**真实 uin** 建配置，预置文件前必须先确认真实 uin（见 §2.2 的警告）。
- **有 `已启动` 但连不上** → 看 `websocketServers` 的字段名。
  配置有 schema 校验，字段名写错会让**整份配置失效**，且不一定会报错。

### 5.4 不装任何库验一遍 OneBot

想绕开抽屉、直接问 NapCat 要数据时，用裸 socket 手写 WebSocket 握手即可
（本机脚本 `.workbuddy/tmp/wstest.py`，带 `Authorization: Bearer <token>`）：

```
3001 OPEN
status: HTTP/1.1 101 Switching Protocols
get_login_info → {"status":"ok","retcode":0,"data":{"user_id":…,"nickname":"…"}}
get_status     → {"status":"ok","retcode":0,"data":{"online":true,"good":true}}
```

**握手能过 ≠ 能收数据**，所以探针一定要发一条真实的 action 并等响应——只测端口开没开会被骗。

### 5.5 重启整套应用之前

`vite.config.ts` 里是 `strictPort: true`，5173 被占着 `npm run dev` 会**直接失败**，
但 Tauri 进程可能还活着、界面是断的，日志里看不出异常。所以重启前先确认三件事：

1. `5173` 空着（否则先杀掉自己起的 vite）
2. `qq-drawer.exe` 没在跑
3. **别误杀 `napcat\node.exe`**——那是 NapCat 本体（有主进程 + worker 两个），杀掉要重新登录

### 5.6 图片落盘失败（日志有 `图片落盘失败`）

```
WARN qq_drawer_lib::media: 图片落盘失败 error=落盘图片失败: …\media\5a\5aae….gif
```

**先看是不是被写保护拦了。** 如果这个进程是从受限环境（沙箱 / 自动化宿主）里起来的，
stderr 里会有这么一段，直接点名：

```
[sandbox] 命令被沙箱拦截，以下操作被拒绝：
  - …\AppData\Local\qq-drawer\media\d2\d28e….jpg.part (读/写 · 拒绝)
```

注意 `EBWebView` 的缓存目录也一起被拒（§1.4 讲的是同一条策略：**对工作区之外的写入要过检查**，
且被拒是 fail-closed）。这种情况下**代码没问题**，换个普通终端启动就正常。
特征是"偶发"：同一张图重试一次往往就成了，所以 `image_cache` 里最终仍然有记录。

真在正常环境里也必现的话，再往杀软/权限上查。

### 5.7 build script 报 `gcc … No such file or directory` / `windres: preprocessing failed`

```
error: failed to run custom build command for `qq-drawer v1.0.0 (R:\Code\QQ Drawer\src-tauri)`
--- stderr
gcc: error: Drawer\src-tauri\target\debug\build\qq-drawer-…\out: No such file or directory
windres: preprocessing failed.
```

**这是路径里的空格，不是缺文件。** 仓库在 `R:\Code\QQ Drawer`，`tauri-winres` 生成
资源时把 `…\QQ Drawer\src-tauri\target\…\out` 传给 `windres` → `gcc`，参数没被引号包住，
被按空格切成两段（报错里那个 `Drawer\src-tauri\…` 就是被切掉前半段的残骸）。

修法：把 `CARGO_TARGET_DIR` 指到**不含空格**的路径再构建：

```powershell
$env:CARGO_TARGET_DIR = "$env:TEMP\qq-drawer-target"   # 只要不含空格，放哪都行
npm run tauri dev
```

`cargo test` / `cargo clippy` 同样会被这个坑影响，所以跑 Rust 命令前也要设。
`.\scripts\dev.ps1` 已经处理好了。

> 只在 workspace 路径含空格时才会出现。换成 `R:\code\qq-drawer` 这种没空格的路径
> 也可以根治，但已经装好的工程不值得为它搬家。

### 5.8 release 链接失败：`error: export ordinal too large: 73741`

```
error: could not compile `qq-drawer` (lib) due to 1 previous error
note: ld.exe: error: export ordinal too large: 73741
      collect2.exe: error: ld returned 1 exit status
```

**这是 cdylib 的锅，不是代码问题。** `Cargo.toml` 的 `[lib] crate-type` 原本是
`["staticlib", "cdylib", "rlib"]`（Tauri 模板给移动端留的）。Windows 上用 MinGW
链接 cdylib 时，`ld` 默认 `--export-all-symbols`，release 开了 fat LTO +
`opt-level="z"` 之后被导出的符号数超过 PE 的 **65535 个序号上限**，于是直接失败。
dev profile 不开 LTO，符号少，所以 `tauri dev` 从没暴露过。

修法：本项目只做 Windows 桌面端，把 `crate-type` 收成 `["rlib"]` 即可（已改）。
`staticlib` 是 iOS 用的、`cdylib` 是 Android 用的，桌面端一个都不需要。

> 如果以后真要出移动端，别把它们加回来——改成给 cdylib 单独限制导出符号
> （`-Wl,--exclude-all-symbols` 或提供 `.def`），而不是让 ld 全量导出。

---

## 开发

### 常用命令

| 命令 | 作用 |
| --- | --- |
| `.\scripts\dev.ps1` | 起完整应用（自动设好 Rust PATH / `CARGO_TARGET_DIR`，免踩 §5.7） |
| `.\scripts\build.ps1` | 打 release 包，产物归拢到 `R:\QQ-Drawer\` |
| `.\scripts\start.bat` | 日常启动：按需拉起 NapCat + 抽屉（见 §4.1） |
| `npm run dev` | Vite 开发服务器（走 mock 后端，脱离 NapCat 调界面） |
| `npm test` | 前端单测（vitest，131 个用例） |
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
│   │   └── account.rs                # "哪些表属于账号"的清单 + 换账号时的整体清空
│   ├── account.rs                    # 登录后判账号归属，换号则清库并通知前端（见「已知限制」10）
│   ├── sched/notify.rs               # 提醒优先级（纯函数）
│   ├── media.rs                      # 图片落盘 / 去重 / LRU / media:// 协议
│   ├── window.rs tray.rs hotkey.rs   # 无边框窗口几何、托盘、全局快捷键
│   ├── cmd.rs                        # 暴露给前端的 IPC 命令
│   └── lib.rs                        # 装配
├── tests/                            # 前端集成测试
├── scripts/                          # 本机工具：dev.ps1 / build.ps1 / start.bat
├── docs/                             # 规格说明书
└── preview/                          # 交互形态预览（静态 HTML）
```

> `scripts/` 下的三个脚本都硬编码了本机路径（`R:\Rust\…`、`R:\NapCat`、`R:\QQ-Drawer`），
> 换机器要改文件顶部的变量。

### 架构上不能破的五条

1. **单一数据源**——只有 Rust 侧的 SQLite 是权威数据。前端不缓存业务状态，只渲染 Rust 推来的快照与增量。
2. **协议隔离**——OneBot 的一切（事件名、字段、消息段）只准出现在 `src-tauri/src/ob/` 里，上层只认 `ob::model` 的规范模型。QQ 协议改版时只动这一层。
3. **窗口与内容解耦**——窗口的尺寸/位置/模糊效果由 Rust 掌握，前端只画内容，**永远不直接改窗口几何**。前端能做的只有三件事：请 Rust 展开、请 Rust 收起、请 Rust 开始拖动（`begin_drag` 里裹了"拖动期间抑制失焦收起"）。**窗口形态以 Rust 广播的 `window_state` 为准**，前端 store 里那份 `expanded` 只是镜像——两份各自为政过一次，代价是"展开尺寸的窗口里画着折叠条，得再点一下才恢复"。
4. **动画不上窗口**——一切视觉动效在 CSS 层（只做 `opacity` / `transform` / `background-color`）。逐帧改窗口尺寸会让 DWM 每帧重算背景模糊，必卡。
5. **虚拟滚动的视口位置只由锚点派生**——`offsetHeight` 量出来的行高、翻页插入的历史，都会让累计高度表 `offsets` 换代；坐标系一换，同一个 `scrollTop` 就指向别的内容了。所以每次换代都要先取锚点（**行 id + 行内偏移**）再反算 `scrollTop`，**不做任何增量加减**（见 `core/vscroll.ts` 的 `anchorKeyAt` / `topForKey`）。围绕它有一串红线，每一条单独都能让滚动抽风：
   - **视口只由「坐标系换代」驱动**（`planViewport`，有单测）。用户的滚动不改行序与行高的引用，必须判成 `hold` 什么都不做——把用户的滚动也当"换代"去校正，就是把他刚滚出来的位置又拽回去。换会话/贴底追加才 `jump-bottom`，其余一律 `pin`（钉住锚点），翻页期间**再贴底也不跟**。
   - **逻辑位置与真实位置同批更新**。`scrollTop` 信号（`range` 的唯一输入）只允许从 `applyTop` 一个口子写，写完顺手同步信号，并记**浏览器夹取后**的真实值——总高度刚变小时我们想写的位置可能越界，记想要的那个值等于让逻辑位置从此与实际错开，渲染窗口就会算出一段根本不在视口附近的行（"快速滚动会飞到很上面的聊天记录，过一会又滚回来"）。
   - **每次换代都要同步量一遍真实行高**（`mergeMeasured`）。新渲染出来的行在 `ResizeObserver` 回调到达之前仍按估值参与计算，用这种坐标系反算的锚点本身就是错的——翻页插入一页偏高行时偏差可达数百像素，先错一帧、等回填再跳回来，用户看到的就是"载入新历史时闪一下、衔接不准"。容器还要显式 `overflow-anchor: none`，否则浏览器自带的 scroll anchoring 会在"上方内容变高"时自己改 `scrollTop`，与锚点补偿叠成双份偏移。
   - **行高估值只学一次**（首屏样本够了就钉死，换会话才重学）。它参与 `padTop` / `padBottom`，跟着样本浮动会让**所有未渲染行**一起变——500 行列表里是几千像素的 `scrollHeight` 突变，滚动条长度跟着跳，渲染窗口边界大幅移动又引发新一波测量，自激之下滚轮直接不动了。
   - **行对象必须复用引用**（`core/grouping.ts` 的 `createRowCache`）。`<For>` 按**引用** diff，行对象每次全新就等于"每次消息追加都重建整个渲染窗口的 DOM"，同上自激。注意 `separator` 是对象，判"没变"要逐字段比——直接 `===` 会恒为 false，让缓存**静默失效**。
   - **程序补偿写入的 `scrollTop` 要和用户的滚动区分开**：浏览器派发的 scroll 事件里分不出这两者，不区分的话补偿会被当成"用户滚到底了"，重新打开贴底跟随。贴底阈值也收得很紧（12px）。翻页等待期间不再重复触发翻页，但**贴底态照常重算**——用户在等待里滚回底部时必须能恢复跟随。
   - 行间距做在 `.row-wrap` 的 `padding` 上而不是父级 flex `gap`（`gap` 不进 `offsetHeight`）；`ResizeObserver` 在组件函数体里建而不是 `onMount`（`ref` 回调比 `onMount` 先跑，在 `onMount` 里建会让首屏的行一个都测不到）。

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
7. **QQ 机器人爱用的 `markdown` 只做了有限支持**（`![alt](url)` 取图、`[文本](url)` 留标签文字）。
   机器人还会发 `json` / `xml` 卡片，那两类仍然只显示 `[卡片消息]`，不解析小程序。
8. **图片落盘偶发失败**：日志出现 `图片落盘失败` 而缓存目录里其实有文件，说明当次
   `write`/`rename` 被瞬时拒绝。实测最常见的原因是**进程从受限/沙箱环境里启动**——
   沙箱会明确拒绝 `%LOCALAPPDATA%\qq-drawer\media\**` 与 `EBWebView` 缓存的写入
   （同一条策略，见 §1.4 与 §5.6）。换个普通终端启动即可。落盘本身没有重试，
   被拒的那条消息会显示 `[图片加载失败]`，但同一张图往往在别的消息或下一次重试里已经缓存好了。
9. **`Ctrl+Alt+M`（静音）可能被别的程序占用**，注册失败只打一条 WARN 就降级，不影响开关折叠条的 `Ctrl+Alt+Q`。
10. **抽屉是单账号的，换 QQ 号登录会清空上一个号的本地记录**。`conversation` / `message` /
    `member_cache` / `mute` / `tombstone` 都不带账号列，所以判定"这次登录的号与上次不同"时，
    这些表整体清空（图片一起清），再按新号重新 seed —— 这就是"换号后上个号的会话不会留在
    界面上"的实现方式。
    判定依据是 meta 里的 `self_id`：**同号重连不清**；拿不到 `self_id` 时**什么都不做**
    （宁可这轮不判，也不能因为一个 0 把好数据清了）；老库从没记过 `self_id` 而有数据时，
    因为归属无从考证，会当作上个号的残留清一次（只在升级后的第一次登录发生）。
    **`settings` 一律不动** —— 连接地址、token、窗口位置、折叠条宽度都不受换号影响。
    对应实现：`src-tauri/src/account.rs` + `src-tauri/src/store/account.rs`。
11. **发消息的"超时"不等于失败，界面会多等一会儿才报错**。带图的消息要先上传，
    上游偶尔 30 秒内回不来 `echo`（旧版按 8 秒超时算失败，于是出现"报错 + 消息其实
    发出去了 + 又多一条重复"）。现在的语义是：
    - `send_message` **不在命令里等上游**，落完乐观条目立刻返回，输入框不会被卡住；
    - 上游超时（`SEND_TIMEOUT_MS = 30s`）**不当失败**，条目保持"在途"，等真实的
      `message_sent` 回执来对账（`store::message::take_pending`，同会话 + 同文本 +
      3 分钟窗口，取最旧一条）—— 回执一到就把乐观条目收掉、换成真实 `message_id`；
    - 只有等满 `ECHO_GRACE_MS = 60s` 仍没有回执，才标成"发送失败 · 点击重试"。
    已知取舍：对账靠"内容 + 时间窗"，因为 OneBot 的回执事件里**没有**能对上我们
    `echo` 的字段。所以含 `@` 的消息如果本地昵称与上游渲染的名字不一致，可能对不上，
    结果是**留下一条重复**（不会丢消息）—— 这是刻意选的偏保守方向。

## 免责声明

本项目为个人自用的第三方工具，与腾讯公司无关，未获得任何形式的官方授权或认可。

不修改、不破解、不注入 QQ 客户端本体，只把第三方协议端（NapCat）暴露的标准 OneBot 接口渲染成一个桌面组件。使用第三方协议端**存在账号被限制登录的风险**，请在了解并接受该风险的前提下自行决定是否使用；不要用于商业用途或任何自动化群发场景。不分发、不销售、不提供在线服务。
