# 第三方源码（vendored）

这两份代码直接编译进 `airplay2dll.dll`，均为 **MIT 许可**，随附原始版权声明。

## `alac.c` / `alac.h` —— ALAC 解码器

- 来源：<https://github.com/philippe44/libraop>（AirConnect 项目的 RAOP 库）
- 原始作者：**David Hammerton**，2005，<http://crazney.net/programs/itunes/alac.html>
- 许可：MIT（完整声明在 `alac.c` 文件头）

iPhone 把本机当作 AirPlay 音箱（纯音频播放）时发送的是 **ALAC**，而协议库自带的解码器
被硬编码成 AAC-ELD（那是屏幕镜像用的格式），因此需要这份解码器。判定依据见
`docs/ffi-contract.md`：SETUP 协商出 `ct=2 spf=352`，其中 352 是 AirPlay ALAC 的标志性
包长（AAC-LC 是 1024，AAC-ELD 是 480）。

### 本地修改

**`decode_frame` 里两处 `outputsamples = readbits(alac, 32)` 增加了上限检查。**

原实现直接采信码流里声明的样本数，而 `allocate_buffers()` 分配的所有内部缓冲都是按
`setinfo_max_samples_per_frame` 计算的。一个畸形（或恶意）音频包声明一个更大的值就会
越过全部缓冲写入 —— 而这些数据来自网络。改动是把它钳制到 `setinfo_max_samples_per_frame`，
搜索 `LOCAL CHANGE` 可定位。

## `dmap_parser.c` / `dmap_parser.h` —— DAAP 元数据解析

- 来源：<https://github.com/philippe44/dmap-parser>
- 原始作者：**Matt Stevens**，2011-2013
- 许可：MIT（原文见 `LICENSE.dmap-parser`）

用于解析 iPhone 通过 RTSP `SET_PARAMETER` 发来的曲目信息（歌名 / 艺人 / 专辑）。
未作修改。
