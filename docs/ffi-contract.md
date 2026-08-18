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

// 交错 PCM，已由协议库的 fdk-aac 解出；传 NULL 表示不要音频
typedef void (*audio_cb)(const uint8_t* pcm, int len, int sample_rate, int channels,
                         int bits_per_sample);

typedef struct {
    const char*  server_name;   // iPhone「屏幕镜像」里显示的设备名
    unsigned int raop_port;     // RAOP（音频）端口
    unsigned int airplay_port;  // AirPlay（镜像）端口 —— iPhone 连接用
    const char*  password;      // NULL/"" = 无 PIN
    int width, height, fps;     // 保留字段，当前忽略（始终用设备原生流）
} mirror_cfg;

int  mirror_start_av(const mirror_cfg* cfg, video_cb vcb, state_cb scb, audio_cb acb);
void mirror_stop(void);
```

> **签名变了就改名**：这个导出曾叫 `mirror_start`（YUV 回调）、`mirror_start_ex`（无音频）。
> 每次参数表变化都换新名字，这样新旧不匹配会在符号查找阶段**响亮失败**，而不是让宿主多压一个
> 参数、被 DLL 当作垃圾指针调用。

`ffi.rs` 里的 `#[repr(C)] MirrorCfg` 必须与上面**逐字节一致**（64 位下指针 8 字节、
int/unsigned 4 字节）。

### `mirror_start_av` 返回码

| 码 | 含义 | `ffi.rs` 给用户的提示 |
| --- | --- | --- |
| `0` | 成功 | — |
| `1` | 已在运行 | 先停止再重新开始 |
| `-1` | 参数无效 | 内部错误 |
| `-2` | 协议栈启动失败 | 检查 Bonjour 服务与防火墙 |
| `-3` | 缺少 Bonjour（`dnssd.dll`） | 提示安装 Apple Bonjour |
| `-4` | 端口被占用 | 提示关闭占用者或换端口 |

`-3` 和 `-4` 由 `Bridge.cpp` 在调用协议栈**之前**主动探测得出 —— 这两种失败用户能自己解决，
值得给出具体提示，而不是笼统的"启动失败"。

> **Bonjour 是硬依赖**：`dnssd_init()` 会 `LoadLibrary("dnssd.dll")` 并解析 7 个符号，
> 没有它就没有 mDNS 广播，iPhone 永远发现不了本机。它**不随本程序分发**（通常随 iTunes 安装）。

### 回调约定

- 两个回调都跑在 DLL 的网络线程上，必须尽快返回，且**不得持锁再调回 DLL**（`mirror_stop`
  会 join 这些线程，持锁会死锁）。
- `video_cb` 的 `data` 只在调用期间有效，返回后立即释放 —— 需要保留必须自行复制。
- `frame_type` 沿用上游 `h264_decode_struct::frame_type` 的语义。注意它**不是**关键帧标志：
  上游 `raop_rtp_mirror.c` 里 `frame_type = 0` 发的是 SPS/PPS 参数集，`= 1` 才是图像数据。

### 音频

音频是**可选的**，默认关闭：`acb` 传 NULL 时 DLL 完全不走 PCM 路径。

与视频不同，音频**确实**经过上游的 `IAirServerCallback::outputAudio`，因此不需要改上游代码。
协议库用内置的 fdk-aac 把 AAC 解成 PCM 再交出来，实测形态是 **16 位有符号小端、480 帧/包
（1920 字节）**，采样率与声道数由解码器上报（`aacDecoder_GetStreamInfo`），随包给出而不是
只报一次 —— 8 字节头相对 2KB 负载可以忽略，却省掉了"格式永不变化"这个假设。

Rust 侧（`ffi.rs::on_audio`）在每块 PCM 前加上小端头
`[u32 sample_rate][u16 channels][u16 bits_per_sample]`，经独立的 Tauri `Channel` 送到前端，
由 `src/lib/pcmPlayer.ts` 用 AudioWorklet 播放。

## 断开检测

iPhone 停止投屏时，协议栈会走到 `raop_rtp_mirror_stop()`，它对**两种情况**都触发
`disconnected` 回调：RTSP `TEARDOWN`（正常停止）和 socket 异常关闭（拔网线/锁屏等）。
因此不需要任何“静默超时”启发式 —— 静止画面不再会被误判为断开。

## 已知坑（都已在 `ffi.rs` 里处理）

- **不要卸载再重载 DLL**：它有全局状态和自己的线程，`FreeLibrary` 后重载会崩/卡。`ffi.rs`
  用 `OnceLock` 只加载一次，靠 `mirror_start_av`/`mirror_stop` 复用，实现停止/重开。
- **加载路径含空格**：旧版本在带空格的目录下会卡死（`mirror_start` 永不返回），根因是随包的
  FFmpeg DLL 是 Cygwin 构建、会拉起 `msys-2.0.dll` 做路径转换。当前 DLL 已不依赖它们，实测
  带空格路径 15ms 加载完成，因此原先的 `resolve_space_free_dll`（8.3 短路径 / 复制到无空格
  暂存目录）已删除。若将来重新引入任何 Cygwin/MSYS 构建的依赖，这个坑会回来。

## 重新编译 DLL

```bash
tools/build-airplay-dll.sh [上游仓库路径]   # 默认 E:/tmp/xenos/AirPlayServer-research，不存在会自动 clone
```

需要 MSYS2 mingw64（`C:\msys64`）。脚本把 `tools/airplay-dll/` 的覆盖层拷进上游树后构建。
覆盖层是两个替换文件：

| 文件 | 作用 |
| --- | --- |
| `Bridge.cpp` | 上面这套 C ABI，含连接/断开状态回调与启动前置检查 |
| `FgAirplayChannel.{h,cpp}` | 去掉 FFmpeg 解码的转发版通道，H.264 原样交给宿主 |

另外对上游打 4 处 sed 补丁：2 处历来必需的编译兼容修正，2 处让启动失败能传出来
（上游 `FgAirplayServer::start()` 无条件 `return 0`，`fgServerStartWithDisplay` 又不看返回值，
于是启动失败的接收器在宿主看来完全正常）。**每处补丁构建时都会校验，锚点失配就直接报错退出**，
不会静默漏打。构建脚本末尾还会自检导出符号，并在 FFmpeg/MSYS 依赖被重新引入时报错。

冒烟测试（不需要手机）：

```bash
gcc -O2 -o smoke.exe tools/airplay-dll/smoke_test.c
smoke.exe "C:\some dir\airplay2dll.dll" 7010 4
```

它检查三件历史上出过问题的事：DLL 能否加载、`mirror_start_av` 会不会卡住、以及带空格路径和
单进程内 start/stop/start/stop 是否都正常。
