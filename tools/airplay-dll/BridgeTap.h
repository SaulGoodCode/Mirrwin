#pragma once

// Internal seam between the channel (which receives the H.264 elementary
// stream) and Bridge.cpp (which owns the host callbacks). Not exported.
//
// `is_codec_config` is upstream's `frame_type == 0`, which despite the name is
// the SPS/PPS parameter-set packet, not a keyframe — see raop_rtp_mirror.c
// where frame_type 0 carries sps_pps and frame_type 1 carries picture data.
void bridge_on_h264(const unsigned char* data, int len, int is_codec_config);
