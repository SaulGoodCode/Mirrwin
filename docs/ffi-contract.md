# 原生协议库集成说明（airplay2dll.dll）

本项目通过 FFI 加载一个原生 C/C++ 协议库 `src-tauri/resources/ffmpeg/airplay2dll.dll`
（基于 [xenos1337/AirPlayServer](https://github.com/xenos1337/AirPlayServer) + 一个 `Bridge.cpp` shim）。
该库负责完整的 AirPlay 协议栈（mDNS 广播、RTSP、FairPlay 配对/解密、RTP），并把解密后的
**H.264 Annex-B 裸流**写入命名管道 `\\.\pipe\AirPlayVideo`。

## 数据流

1. Rust（`src-tauri/src/ffi.rs`）在运行时 `dlopen` 该 DLL，通过 C ABI 启动接收器。
2. DLL 把 H.264 mux 到命名管道；Rust 的 `spawn_pipe_forwarder` 读取该管道，经 Tauri
   二进制 `Channel` 把 H.264 分片转发给前端。
3. 前端（`src/lib/h264Decoder.ts`）用 WebCodecs `VideoDecoder` 硬件解码并绘制到 `<canvas>`。

> 注意：`Bridge.cpp` 还导出了一个 `frame_cb`（解码后 YUV 回调）接口，但**当前预编译版本
> 从不调用它**（实测 `on_frame` 不触发），视频只经命名管道输出。`ffi.rs` 仍传入一个合法的
> `frame_cb`（`mirror_start` 要求非空），它只是个日志桩。

## C ABI（`Bridge.cpp` 导出，`ffi.rs` 按此声明）

```c
typedef void (*frame_cb)(const uint8_t* y, const uint8_t* u, const uint8_t* v,
                         int width, int height, int stride_y, int stride_u, int stride_v);

typedef struct {
    const char*  server_name;   // iPhone「屏幕镜像」里显示的设备名
    unsigned int raop_port;     // RAOP（音频）端口
    unsigned int airplay_port;  // AirPlay（镜像）端口 —— iPhone 连接用
    const char*  password;      // NULL/"" = 无 PIN
    int width, height, fps;     // 请求的分辨率/帧率（0 = 原生）
} mirror_cfg;

int  mirror_start(const mirror_cfg* cfg, frame_cb cb);  // 返回 0 表示成功
void mirror_stop(void);
```

`ffi.rs` 里的 `#[repr(C)] MirrorCfg` 与 `FrameCb` 必须与上面**逐字节一致**（64 位下指针 8
字节、int/unsigned 4 字节，`frame_cb` 无尾随 userdata 参数）。

## 已知坑（都已在 `ffi.rs` 里处理）

- **加载路径不能含空格**：DLL 依赖的 MSYS2/FFmpeg 运行时在带空格的目录下会卡死
  （`mirror_start` 永不返回）。`resolve_space_free_dll` 会先把 DLL 目录转成 8.3 短路径
  （或复制到无空格暂存目录）再加载。
- **不要卸载再重载 DLL**：它有全局状态和自己的线程，`FreeLibrary` 后重载会崩/卡。`ffi.rs`
  用 `OnceLock` 只加载一次，靠 `mirror_start`/`mirror_stop` 复用，实现停止/重开。
- **断开检测**：该 DLL 在 iPhone 停止投屏时不关闭管道，只是不再发数据，因此靠“静默超时”
  判断断开（见 `spawn_pipe_forwarder`）。

## 重新编译 DLL

预编译好的 DLL 已随仓库提供于 `src-tauri/resources/ffmpeg/`，一般无需自行编译。若要重编，
见 `tools/build-airplay-dll.sh`（基于 MSYS2 / MinGW-w64，克隆 xenos1337/AirPlayServer 后运行）。

运行时必需的依赖 DLL（均为 `airplay2dll.dll` 的**静态导入**，加载时必须存在，即便解码在前端
WebCodecs 完成）：`avcodec-58` / `avutil-56` / `swscale-5` / `libwinpthread-1` / `msys-2.0`。
