// BridgeAudio.cpp : audio decode for the receiver.
//
// AirPlayServerLib decodes exactly one format: AAC-ELD, from a hardcoded
// AudioSpecificConfig in raop_buffer.c. That is what screen mirroring sends, so
// mirroring worked and nothing else did. An audio-only session — picking this
// machine in the phone's AirPlay *audio* list — negotiates ALAC instead, which
// the diagnostic log made plain:
//
//     AUDIO SETUP negotiated: audioFormat=262144 ct=2 spf=352
//     aacDecoder_DecodeFrame error : 0x4002   (x979, every single frame)
//
// spf=352 is the giveaway: that is ALAC's packet size in AirPlay (AAC-LC uses
// 1024, AAC-ELD 480). So this file owns the decode and dispatches on what the
// SETUP negotiated, with ALAC handled by the vendored decoder in vendor/alac.c.
//
// raop_buffer.c calls bridge_decode_audio() with the AES-decrypted payload; the
// build script patches that one block. Everything else stays upstream's.

#include <windows.h>

#include <atomic>
#include <cstdint>
#include <cstring>
#include <string>
#include <mutex>

#include "BridgeTap.h"

extern "C" {
#include "vendor/alac.h"
#include "vendor/dmap_parser.h"
}

// fdk-aac, for the mirroring path this file also has to keep working.
#include "fdk-aac/libAACdec/include/aacdecoder_lib.h"

// ── negotiated format ────────────────────────────────────────────────────
// Set from the RTSP SETUP handler before any audio packet arrives, read on the
// RTP thread. `ct` is the AirPlay compression type; 2 is ALAC.

#define AUDIO_CT_ALAC 2

static std::mutex g_audio_mutex;
static int g_ct = 0;
static int g_spf = 0;
static alac_file* g_alac = nullptr;

/// Tear down the ALAC decoder. Caller holds g_audio_mutex.
static void alac_release_locked() {
    if (g_alac) {
        delete_alac(g_alac);
        g_alac = nullptr;
    }
}

/// Build an ALAC decoder for AirPlay's stream shape.
///
/// AirPlay 1 carries these as the SDP `fmtp` line and libraop feeds them
/// straight in; AirPlay 2 has no SDP, but the values are fixed for the format
/// — `96 352 0 16 40 10 14 2 255 0 0 44100` — and the frame count is confirmed
/// by the `spf` the SETUP negotiated. Caller holds g_audio_mutex.
static void alac_create_locked(int frames_per_packet) {
    alac_release_locked();
    alac_file* alac = create_alac(16, 2);
    if (!alac) {
        bridge_log("audio: could not create the ALAC decoder");
        return;
    }
    alac->setinfo_max_samples_per_frame = frames_per_packet > 0 ? frames_per_packet : 352;
    alac->setinfo_7a = 0;
    alac->setinfo_sample_size = 16;
    alac->setinfo_rice_historymult = 40;
    alac->setinfo_rice_initialhistory = 10;
    alac->setinfo_rice_kmodifier = 14;
    alac->setinfo_7f = 2;
    alac->setinfo_80 = 255;
    alac->setinfo_82 = 0;
    alac->setinfo_86 = 0;
    alac->setinfo_8a_rate = 44100;
    allocate_buffers(alac);
    g_alac = alac;
    bridge_log("audio: ALAC decoder ready (%d frames/packet)", alac->setinfo_max_samples_per_frame);
}

extern "C" void bridge_set_audio_codec(int ct, int spf) {
    std::lock_guard<std::mutex> guard(g_audio_mutex);
    g_ct = ct;
    g_spf = spf;
    if (ct == AUDIO_CT_ALAC) {
        alac_create_locked(spf);
    } else {
        alac_release_locked();
        bridge_log("audio: using the AAC-ELD path (ct=%d spf=%d)", ct, spf);
    }
    // An audio SETUP *is* the start of an audio session, so the host learns
    // about it here rather than needing another patch upstream.
    bridge_on_state_text(BRIDGE_EVENT_AUDIO_START, "", "");
}

extern "C" void bridge_on_audio_stopped(void) {
    {
        std::lock_guard<std::mutex> guard(g_audio_mutex);
        if (!g_ct && !g_spf) {
            return;  // no session was running; nothing to report
        }
        alac_release_locked();
        g_ct = 0;
        g_spf = 0;
    }
    bridge_log("audio: session stopped");
    bridge_on_state_text(BRIDGE_EVENT_AUDIO_END, "", "");
}

extern "C" void bridge_reset_audio_codec(void) {
    std::lock_guard<std::mutex> guard(g_audio_mutex);
    alac_release_locked();
    g_ct = 0;
    g_spf = 0;
}

// ── decode ───────────────────────────────────────────────────────────────

/// Decode one AES-decrypted audio packet to interleaved PCM.
///
/// Returns the number of PCM bytes written, and reports the stream shape so the
/// caller can pass it on unchanged. Called on AirPlayServerLib's RTP thread.
extern "C" int bridge_decode_audio(void* aac_handle, const unsigned char* in, int in_len,
                                   void* out, int out_size, unsigned int* sample_rate,
                                   unsigned short* channels, unsigned short* bits_per_sample) {
    if (!in || in_len <= 0 || !out || out_size <= 0) {
        return 0;
    }

    {
        std::lock_guard<std::mutex> guard(g_audio_mutex);
        if (g_alac) {
            int written = 0;
            // decode_frame writes interleaved 16-bit samples and reports bytes.
            decode_frame(g_alac, const_cast<unsigned char*>(in), out, &written);
            if (written < 0 || written > out_size) {
                bridge_log("audio: ALAC produced %d bytes for a %d byte buffer, dropping",
                           written, out_size);
                return 0;
            }
            if (sample_rate) *sample_rate = 44100;
            if (channels) *channels = 2;
            if (bits_per_sample) *bits_per_sample = 16;
            return written;
        }
    }

    // AAC-ELD: what screen mirroring sends, decoded by the library's own
    // decoder exactly as upstream did.
    HANDLE_AACDECODER handle = (HANDLE_AACDECODER)aac_handle;
    if (!handle) {
        return 0;
    }
    UINT pkt_size = (UINT)in_len;
    UINT valid_size = (UINT)in_len;
    UCHAR* input_buf[1] = {const_cast<UCHAR*>((const UCHAR*)in)};
    AAC_DECODER_ERROR ret = aacDecoder_Fill(handle, input_buf, &pkt_size, &valid_size);
    if (ret != AAC_DEC_OK) {
        bridge_log("audio: aacDecoder_Fill error 0x%x", ret);
    }
    ret = aacDecoder_DecodeFrame(handle, (INT_PCM*)out, out_size / (int)sizeof(INT_PCM), 0);
    if (ret != AAC_DEC_OK) {
        bridge_log("audio: aacDecoder_DecodeFrame error 0x%x", ret);
        return 0;
    }

    CStreamInfo* info = aacDecoder_GetStreamInfo(handle);
    if (!info || info->numChannels <= 0 || info->frameSize <= 0) {
        return 0;
    }
    if (sample_rate) *sample_rate = (unsigned int)info->sampleRate;
    if (channels) *channels = (unsigned short)info->numChannels;
    if (bits_per_sample) *bits_per_sample = 16;
    return info->frameSize * info->numChannels * (int)sizeof(INT_PCM);
}

// ── track metadata ───────────────────────────────────────────────────────
// The phone sends DAAP over RTSP SET_PARAMETER while playing. Upstream's
// FgAirplayServer::audio_set_metadata is an empty function because
// IAirServerCallback has no metadata method to forward to, so the build script
// patches it to land here instead.

namespace {

struct MetaFields {
    std::string title;
    std::string artist;
    std::string album;
};

void on_dmap_string(void* ctx, const char* code, const char* /*name*/, const char* buf,
                    size_t len) {
    if (!ctx || !code || !buf || len == 0) {
        return;
    }
    MetaFields* out = static_cast<MetaFields*>(ctx);
    std::string value(buf, len);
    // DAAP content codes: item name, artist, album.
    if (strcmp(code, "minm") == 0) {
        out->title = value;
    } else if (strcmp(code, "asar") == 0) {
        out->artist = value;
    } else if (strcmp(code, "asal") == 0) {
        out->album = value;
    }
}

}  // namespace

extern "C" void bridge_on_metadata(const void* buffer, int len) {
    if (!buffer || len <= 0) {
        return;
    }
    MetaFields fields;
    dmap_settings settings;
    memset(&settings, 0, sizeof(settings));
    settings.on_string = on_dmap_string;
    settings.ctx = &fields;

    if (dmap_parse(&settings, (const char*)buffer, (size_t)len) != 0) {
        bridge_log("audio: could not parse %d bytes of DAAP metadata", len);
        return;
    }
    if (fields.title.empty() && fields.artist.empty() && fields.album.empty()) {
        return;
    }
    bridge_log("audio: now playing '%s' by '%s' (%s)", fields.title.c_str(),
               fields.artist.c_str(), fields.album.c_str());

    // Album rides along with the artist: state_cb carries two strings, and
    // splitting on a delimiter would break on a title that contains it.
    bridge_on_state_text(BRIDGE_EVENT_METADATA, fields.title.c_str(), fields.artist.c_str());
}

/// Album artwork from SET_PARAMETER. Upstream's audio_set_coverart is another
/// empty function; the build script patches it to land here. The bytes are the
/// image exactly as the phone sent it (JPEG in practice) and are only valid for
/// the duration of the call.
extern "C" void bridge_on_coverart(const void* buffer, int len) {
    if (!buffer || len <= 0) {
        // A track with no artwork sends an empty payload rather than nothing,
        // which is the signal to drop whatever cover is on screen.
        bridge_emit_artwork(nullptr, 0);
        return;
    }
    bridge_log("audio: cover art, %d bytes", len);
    bridge_emit_artwork((const unsigned char*)buffer, len);
}
