#pragma once

// Internal seams between the overlay files and the upstream tree. Nothing here
// is exported from the DLL.

#ifdef __cplusplus
extern "C" {
#endif

// Event codes carried by the host's state_cb. Codes are only ever appended:
// the host ignores ones it does not know, so a newer DLL against an older host
// degrades to "that event never happens" rather than misbehaving.
#define BRIDGE_EVENT_CONNECTED     0  // mirroring started   (name, device id)
#define BRIDGE_EVENT_DISCONNECTED  1  // mirroring stopped   (name, device id)
#define BRIDGE_EVENT_AUDIO_START   2  // audio-only session started
#define BRIDGE_EVENT_AUDIO_END     3  // audio-only session ended
#define BRIDGE_EVENT_METADATA      4  // (track title, artist)
#define BRIDGE_EVENT_VOLUME        5  // (volume in dB as text, "")

/// Raise one of the events above to the host. Safe to call with nulls.
void bridge_on_state_text(int event, const char* a, const char* b);

/// Called by FgAirplayChannel with each H.264 access unit.
///
/// `is_codec_config` is upstream's `frame_type == 0`, which despite the name is
/// the SPS/PPS parameter-set packet, not a keyframe — see raop_rtp_mirror.c
/// where frame_type 0 carries sps_pps and frame_type 1 carries picture data.
void bridge_on_h264(const unsigned char* data, int len, int is_codec_config);

/// Record the audio format the RTSP SETUP negotiated. Called from the upstream
/// SETUP handler before any audio packet arrives.
void bridge_set_audio_codec(int ct, int spf);

/// Called when the upstream audio RTP session stops, however it stops.
void bridge_on_audio_stopped(void);

/// Parse a DAAP metadata blob from SET_PARAMETER and raise it to the host.
void bridge_on_metadata(const void* buffer, int len);

/// Hand the host the album artwork the phone sent (JPEG or PNG bytes).
void bridge_on_coverart(const void* buffer, int len);

/// Deliver artwork bytes to whatever the host registered. Implemented in
/// Bridge.cpp, which owns the host callbacks.
void bridge_emit_artwork(const unsigned char* data, int len);

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
