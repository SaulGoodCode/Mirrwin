#pragma once

// Internal seams between the overlay files and the upstream tree. Nothing here
// is exported from the DLL.

#ifdef __cplusplus
extern "C" {
#endif

/// Called by FgAirplayChannel with each H.264 access unit.
///
/// `is_codec_config` is upstream's `frame_type == 0`, which despite the name is
/// the SPS/PPS parameter-set packet, not a keyframe — see raop_rtp_mirror.c
/// where frame_type 0 carries sps_pps and frame_type 1 carries picture data.
void bridge_on_h264(const unsigned char* data, int len, int is_codec_config);

/// Record the audio format the RTSP SETUP negotiated. Called from the upstream
/// SETUP handler before any audio packet arrives.
void bridge_set_audio_codec(int ct, int spf);

/// Drop any decoder built for the previous session.
void bridge_reset_audio_codec(void);

/// Decode one AES-decrypted audio packet to interleaved PCM, dispatching on the
/// negotiated format. Returns PCM bytes written. Called from raop_buffer.c.
int bridge_decode_audio(void* aac_handle, const unsigned char* in, int in_len, void* out,
                        int out_size, unsigned int* sample_rate, unsigned short* channels,
                        unsigned short* bits_per_sample);

/// Append a line to the opt-in diagnostic log (AIRPLAY_BRIDGE_LOG). A no-op
/// when the variable is unset, so it is safe to call from decode paths.
void bridge_log(const char* fmt, ...);

#ifdef __cplusplus
}
#endif
