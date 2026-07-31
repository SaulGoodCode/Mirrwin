# Mirrwin

在 Windows 上接收手机投屏的桌面应用。画面直接渲染在应用窗口内，支持截图与录制。

当前支持 **iPhone / iPad 的 AirPlay 屏幕镜像**，后续计划加入 **Android 投屏**。基于 **Tauri 2 + Vue 3 + Rust**，视频通过浏览器原生 **WebCodecs** 硬件解码，无需外挂播放器窗口。

---

## ✨ 功能特性

- 📱 **一键接收**：iPhone 在「控制中心 → 屏幕镜像」选择本设备即可投屏
- 🖼️ **窗口内渲染**：画面直接绘制在应用内的 `<canvas>`，自动适配竖屏 / 横屏，无独立弹窗
- 📸 **截图**：一键保存当前画面为 PNG
- 🎥 **录制**：录制投屏画面为 WebM
- 📂 **保存目录**：默认系统「下载」目录，可自选并一键在资源管理器中打开
- 🔴 **状态指示**：已停止 / 接收中 / 投屏中；iPhone 断开后自动回到「接收中」

---

## 🔧 工作原理

```
iPhone ──AirPlay(mDNS/RTSP/FairPlay/RTP)──▶ airplay2dll.dll
                                                │  解出 H.264 (Annex-B)
                                                ▼
                                   \\.\pipe\AirPlayVideo (命名管道)
                                                │
                     Rust 后端读取管道，经 Tauri 二进制 Channel 转发
                                                ▼
                        前端 WebCodecs VideoDecoder (硬件解码 H.264)
                                                ▼
                                   drawImage → <canvas> 渲染
```

1. **协议栈**：预编译的原生库 `airplay2dll.dll`（基于 [xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer)）负责 AirPlay 协议（设备广播、RTSP 握手、FairPlay 配对、RTP 接收与解密），并把 H.264 裸流写入命名管道。
2. **转发**：Rust 后端（`src-tauri/src/ffi.rs`）读取该管道，把 H.264 分片通过 Tauri `Channel` 高效推送到前端（二进制，无 base64 开销）。
3. **解码渲染**：前端（`src/lib/h264Decoder.ts`）用 WebView2 内置的 **WebCodecs** 硬件解码 H.264，逐帧 `drawImage` 到画布。

> 这样做的好处：视频作为普通 DOM 画布渲染，天然与界面合成，不存在跨进程窗口嵌入（`SetParent`）的层级/缩放问题，也无需附带 `ffplay.exe` 等外部播放器。

---

## 📦 环境要求

- **Windows 10 / 11 (x64)**
- **WebView2 运行时**：较新版本（内置 Chromium，需支持 WebCodecs；现代 Windows 一般已预装，Edge 更新即会更新）
- **原生依赖**：`src-tauri/resources/ffmpeg/` 下的 6 个 DLL（详见下方说明），仓库已包含
- iPhone / iPad 与电脑处于**同一局域网**

---

## 🚀 开发与构建

### 前置依赖

- [Node.js](https://nodejs.org/)（含 npm）
- [Rust 工具链](https://rustup.rs/)
- [Tauri 2 前置环境](https://tauri.app/start/prerequisites/)（WebView2、MSVC 生成工具等）

### 运行

```bash
# 安装前端依赖
npm install

# 开发模式（热重载）
npm run tauri dev

# 打包为安装程序 (NSIS)
npm run tauri build
```

---

## 🕹️ 使用方法

1. 启动应用，点击 **开始接收**（顶部状态变为「接收中」）。
2. iPhone 打开 **控制中心 → 屏幕镜像**，选择设备（默认名 `AirPlay Mirror`）。
3. 连接成功后画面出现，状态变为 **投屏中**。
4. 可随时 **截图** / **录制**；在 **设置** 中修改设备名、端口与保存目录。

---

## 🗂️ 项目结构

```
airplay-mirror/                    # 项目根目录
├── src/                          # Vue 3 前端
│   ├── components/
│   │   ├── MirrorCanvas.vue       # 视频渲染 + 截图 / 录制
│   │   ├── Dashboard.vue          # 主界面布局
│   │   ├── StatusBar.vue          # 顶部状态栏
│   │   └── SettingsDialog.vue     # 设置对话框
│   ├── composables/useReceiver.ts # 状态、事件与帧通道管理
│   └── lib/h264Decoder.ts         # WebCodecs H.264 (Annex-B) 解码器
├── src-tauri/                    # Rust 后端 (Tauri 2)
│   ├── src/
│   │   ├── ffi.rs                 # 加载 DLL、读取命名管道并转发 H.264
│   │   ├── commands.rs            # Tauri 命令：开始/停止、截图、录制、打开目录
│   │   ├── state.rs               # 应用共享状态
│   │   └── lib.rs                 # 入口与命令注册
│   └── resources/ffmpeg/          # airplay2dll.dll 及其运行时依赖 DLL
└── docs/ffi-contract.md           # DLL 的 C ABI 说明
```

---

## ⚠️ 说明与限制

- **`resources/ffmpeg/` 下的 6 个 DLL 均为必需**，缺一不可（否则协议库无法加载）：
  | DLL | 作用 |
  | --- | --- |
  | `airplay2dll.dll` | AirPlay 协议库（本程序直接加载） |
  | `avcodec-58.dll` / `avutil-56.dll` / `swscale-5.dll` | `airplay2dll.dll` 的静态导入依赖（FFmpeg，MSYS2 构建） |
  | `libwinpthread-1.dll` | `airplay2dll.dll` 的线程运行时依赖 |
  | `msys-2.0.dll` | 上述 FFmpeg DLL 的运行时依赖 |

  > 说明：虽然实际解码由前端 WebCodecs 完成，但 `airplay2dll.dll` **在链接层静态引用**了 FFmpeg，Windows 加载时会解析整条依赖链，因此这些 DLL 必须存在。
- 协议库为**预编译二进制**；如需自行编译，参考 `docs/ffi-contract.md` 与 [xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer)。
- AirPlay / FairPlay 为 Apple 私有协议，本项目仅供**学习与个人使用**。

---

## 🙏 致谢与许可

- AirPlay 协议库：[xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer)（MIT）
- 随附的 FFmpeg 动态库（`avcodec` / `avutil` / `swscale`）遵循 **FFmpeg 的 LGPL/GPL 许可**，为 MSYS2/MinGW 构建产物；分发时请遵守其许可条款。
- 本项目自身建议根据需要补充 `LICENSE` 文件。
