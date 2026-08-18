# 原生协议库集成说明（airplay2dll.dll）

本项目通过 FFI 加载原生 C/C++ 协议库 `src-tauri/resources/airplay/airplay2dll.dll`，它基于
[xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer) 加上本仓库
`tools/airplay-dll/` 里的覆盖层构建。该库负责完整的 AirPlay 协议栈（mDNS 广播、RTSP、
FairPlay 配对/解密、RTP），并把解密后的 **H.264 Annex-B 裸流**通过回调直接交给宿主。

## 数据流

1. Rust（`src-tauri/src/ffi.rs`）在运行时 `dlopen` 该 DLL，通过 C ABI 启动接收器。
2. DLL 每收到一个访问单元就调用 `video_cb`；Rust 把字节经 Tauri 二进制 `Channel` 转发给前端。
3. 前端（`src/lib/h264Decoder.ts`）用 WebCodecs `VideoDecoder` 硬件解码并绘制到 `<canvas>`。
4. 设备开始/停止投屏时，DLL 调用 `state_cb`；Rust 据此发出 `device_connected` /
   `video_ended` 事件并更新状态。

DLL 内部不解码视频，因此**不依赖 FFmpeg**；运行时只需要同目录下的 `libwinpthread-1.dll`。

## C ABI（`tools/airplay-dll/Bridge.cpp` 导出，`ffi.rs` 按此声明）

```c
// frame_type: 0 = SPS/PPS 参数集，1 = 图像数据
typedef void (*video_cb)(const uint8_t* data, int len, int frame_type);

// event: 0 = connected, 1 = disconnected
typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);

typedef struct {
    const char*  server_name;   // iPhone「屏幕镜像」里显示的设备名
    unsigned int raop_port;     // RAOP（音频）端口
    unsigned int airplay_port;  // AirPlay（镜像）端口 —— iPhone 连接用
    const char*  password;      // NULL/"" = 无 PIN
    int width, height, fps;     // 保留字段，当前忽略（始终用设备原生流）
} mirror_cfg;

int  mirror_start_ex(const mirror_cfg* cfg, video_cb vcb, state_cb scb);  // 0 = 成功，1 = 已在运行
void mirror_stop(void);
```

`ffi.rs` 里的 `#[repr(C)] MirrorCfg` 必须与上面**逐字节一致**（64 位下指针 8 字节、
int/unsigned 4 字节）。

### 回调约定

- 两个回调都跑在 DLL 的网络线程上，必须尽快返回，且**不得持锁再调回 DLL**（`mirror_stop`
  会 join 这些线程，持锁会死锁）。
- `video_cb` 的 `data` 只在调用期间有效，返回后立即释放 —— 需要保留必须自行复制。
- `frame_type` 沿用上游 `h264_decode_struct::frame_type` 的语义。注意它**不是**关键帧标志：
  上游 `raop_rtp_mirror.c` 里 `frame_type = 0` 发的是 SPS/PPS 参数集，`= 1` 才是图像数据。

## 断开检测

iPhone 停止投屏时，协议栈会走到 `raop_rtp_mirror_stop()`，它对**两种情况**都触发
`disconnected` 回调：RTSP `TEARDOWN`（正常停止）和 socket 异常关闭（拔网线/锁屏等）。
因此不需要任何“静默超时”启发式 —— 静止画面不再会被误判为断开。

## 已知坑（都已在 `ffi.rs` 里处理）

- **不要卸载再重载 DLL**：它有全局状态和自己的线程，`FreeLibrary` 后重载会崩/卡。`ffi.rs`
  用 `OnceLock` 只加载一次，靠 `mirror_start_ex`/`mirror_stop` 复用，实现停止/重开。
- **加载路径含空格**：旧版本在带空格的目录下会卡死（`mirror_start` 永不返回），根因是随包的
  FFmpeg DLL 是 Cygwin 构建、会拉起 `msys-2.0.dll` 做路径转换。当前 DLL 已不依赖它们，实测
  带空格路径 15ms 加载完成，因此原先的 `resolve_space_free_dll`（8.3 短路径 / 复制到无空格
  暂存目录）已删除。若将来重新引入任何 Cygwin/MSYS 构建的依赖，这个坑会回来。

## 重新编译 DLL

```bash
tools/build-airplay-dll.sh [上游仓库路径]   # 默认 E:/tmp/xenos/AirPlayServer-research，不存在会自动 clone
```

需要 MSYS2 mingw64（`C:\msys64`）。脚本把 `tools/airplay-dll/` 的覆盖层拷进上游树后构建，
**不修改上游源码**（除两处历来必需的编译兼容 sed 补丁）。覆盖层只有两块：

| 文件 | 作用 |
| --- | --- |
| `Bridge.cpp` | 上面这套 C ABI，含连接/断开状态回调 |
| `FgAirplayChannel.{h,cpp}` | 去掉 FFmpeg 解码的转发版通道，H.264 原样交给宿主 |

构建脚本末尾会自检导出符号，并在 FFmpeg/MSYS 依赖被重新引入时报错退出。

冒烟测试（不需要手机）：

```bash
gcc -O2 -o smoke.exe tools/airplay-dll/smoke_test.c
smoke.exe "C:\some dir\airplay2dll.dll" 7010 4
```

它检查三件历史上出过问题的事：DLL 能否加载、`mirror_start_ex` 会不会卡住、以及带空格路径和
单进程内 start/stop/start/stop 是否都正常。
