// Bridge.cpp : C ABI bridge between AirPlayServerLib and the Rust/Tauri host.
//
//   typedef void (*video_cb)(const uint8_t* data, int len, int frame_type);
//   typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);
//
//   int  mirror_start_av(const mirror_cfg* cfg, video_cb vcb, state_cb scb, audio_cb acb);
//   void mirror_stop(void);
//
// The host receives the H.264 elementary stream as it arrives and decodes it
// itself, so nothing here touches FFmpeg and no named pipe is involved. Audio
// is optional: pass a null audio_cb and the PCM path stays idle.
// `state_cb` surfaces the receiver's connect/disconnect edges, which the
// protocol stack already tracks: AirPlayServerLib raises them from
// raop_rtp_start_mirror / raop_rtp_mirror_stop, so a disconnect is reported on
// both an RTSP TEARDOWN and an abrupt socket close.
//
// All callbacks run on AirPlayServerLib's network threads and must return
// promptly. Their buffers are owned by the caller and freed as soon as the call
// returns — copy anything you need to keep.

#include <winsock2.h>
#include <windows.h>

#include "Airplay2Head.h"
#include "BridgeTap.h"

#include <atomic>
#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <mutex>

#ifdef __cplusplus
extern "C" {
#endif

// frame_type: 0 = SPS/PPS parameter sets, 1 = picture data.
typedef void (*video_cb)(const uint8_t* data, int len, int frame_type);

// event: 0 = connected, 1 = disconnected.
typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);

// Interleaved PCM, already decoded from AAC by the library's fdk-aac decoder.
// In practice 16-bit signed little-endian, 480 frames per call, at whatever the
// stream reports. Pass a null audio_cb to mirror_start_av to leave audio off,
// which is the default: mirroring is usable without it and users who only want
// the picture should not pay for the extra traffic.
typedef void (*audio_cb)(const uint8_t* pcm, int len, int sample_rate, int channels,
                         int bits_per_sample);

// Album artwork as the phone sent it — JPEG in practice. Registered separately
// from mirror_start_av so adding it needs no change to that signature: a host
// that wants artwork looks the setter up and fails loudly if the DLL predates
// it, which a fourth parameter could not have done.
typedef void (*art_cb)(const uint8_t* data, int len);

typedef struct mirror_cfg {
    const char* server_name;
    unsigned int raop_port;
    unsigned int airplay_port;
    const char* password;
    int width;   // reserved (0 = native)
    int height;  // reserved (0 = native)
    int fps;     // reserved (0 = native)
} mirror_cfg;

#ifdef __cplusplus
}
#endif

// Return codes for mirror_start_av. The host turns these into messages, so a
// failure that a user can act on gets its own code rather than a generic one.
#define MIRROR_OK              0
#define MIRROR_ALREADY_RUNNING 1
#define MIRROR_ERR_ARGS        (-1)
#define MIRROR_ERR_START       (-2)
#define MIRROR_ERR_NO_BONJOUR  (-3)
#define MIRROR_ERR_PORT_BUSY   (-4)

// ── preflight ────────────────────────────────────────────────────────────
// Upstream reports every startup failure the same way: not at all. Its
// FgAirplayServer::start() returned 0 unconditionally and the export ignored
// the result, so a receiver that never came up still looked started. The build
// script patches those two spots, but a bare error code cannot tell a user what
// to do — so the two failures that are actually actionable are detected here
// and named.

/// Whether Apple's Bonjour is usable. AirPlayServerLib's `dnssd_init()`
/// LoadLibrary's `dnssd.dll` and resolves exactly these symbols; without them
/// there is no mDNS advertisement and the phone never discovers this machine.
/// Bonjour is not bundled with the app — it arrives with iTunes — so on a clean
/// Windows install this is the first thing that fails.
static bool bonjour_available(void) {
    HMODULE m = LoadLibraryA("dnssd.dll");
    if (!m) {
        return false;
    }
    static const char* const kRequired[] = {
        "DNSServiceRegister", "DNSServiceRefDeallocate", "TXTRecordCreate",
        "TXTRecordSetValue", "TXTRecordGetLength", "TXTRecordGetBytesPtr",
        "TXTRecordDeallocate",
    };
    bool ok = true;
    for (size_t i = 0; i < sizeof(kRequired) / sizeof(kRequired[0]); i++) {
        if (!GetProcAddress(m, kRequired[i])) {
            ok = false;
            break;
        }
    }
    // dnssd_init takes its own reference moments later; drop ours.
    FreeLibrary(m);
    return ok;
}

/// Whether a TCP port can still be bound. `raop_start`/`airplay_start` bind the
/// exact port they are given and fail outright if it is taken — commonly by a
/// second copy of this app or another AirPlay receiver.
static bool port_free(unsigned short port) {
    static std::once_flag ws_once;
    static bool ws_ready = false;
    std::call_once(ws_once, []() {
        WSADATA data;
        // Deliberately never paired with WSACleanup: the reference is held for
        // the process, so nothing here can tear Winsock out from under the
        // protocol stack.
        ws_ready = (WSAStartup(MAKEWORD(2, 2), &data) == 0);
    });
    if (!ws_ready) {
        return true;  // cannot tell; let the stack try
    }

    SOCKET s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (s == INVALID_SOCKET) {
        return true;
    }
    struct sockaddr_in addr;
    memset(&addr, 0, sizeof(addr));
    addr.sin_family = AF_INET;
    addr.sin_addr.s_addr = INADDR_ANY;
    addr.sin_port = htons(port);
    // No SO_REUSEADDR on purpose: on Windows it would let this bind land next
    // to an existing listener and hide the very conflict being probed for.
    bool ok = (bind(s, (struct sockaddr*)&addr, sizeof(addr)) == 0);
    closesocket(s);
    return ok;
}

// ── opt-in diagnostics ───────────────────────────────────────────────────
// Off unless AIRPLAY_BRIDGE_LOG names a file. Lifecycle events only: an
// earlier build of this DLL logged every frame through a fopen/fclose pair on
// a path hardcoded to one developer's home directory.

static FILE* g_logf = nullptr;
static std::once_flag g_log_once;
static std::mutex g_log_mutex;

void bridge_log(const char* fmt, ...) {
    std::call_once(g_log_once, []() {
        const char* path = getenv("AIRPLAY_BRIDGE_LOG");
        if (path && *path) {
            g_logf = fopen(path, "a");
            if (g_logf) {
                fprintf(g_logf, "\n===== NEW SESSION =====\n");
            }
        }
    });
    if (!g_logf) {
        return;
    }
    std::lock_guard<std::mutex> guard(g_log_mutex);
    va_list args;
    va_start(args, fmt);
    fprintf(g_logf, "[%lu] ", (unsigned long)GetTickCount64());
    vfprintf(g_logf, fmt, args);
    fputc('\n', g_logf);
    va_end(args);
    fflush(g_logf);
}

// ── host callbacks ───────────────────────────────────────────────────────
// Atomic because the network threads read them while mirror_stop() clears
// them. Both are cleared before the server is torn down, so a callback racing
// a stop sees null rather than a callback the host has already dropped.

static std::atomic<video_cb> g_video_cb{nullptr};
static std::atomic<state_cb> g_state_cb{nullptr};
static std::atomic<audio_cb> g_audio_cb{nullptr};
static std::atomic<art_cb> g_art_cb{nullptr};

void bridge_on_h264(const unsigned char* data, int len, int is_codec_config) {
    video_cb cb = g_video_cb.load(std::memory_order_acquire);
    if (cb && data && len > 0) {
        cb((const uint8_t*)data, len, is_codec_config ? 0 : 1);
    }
}

void bridge_emit_artwork(const unsigned char* data, int len) {
    art_cb cb = g_art_cb.load(std::memory_order_acquire);
    if (cb && data && len > 0) {
        cb((const uint8_t*)data, len);
    }
}

void bridge_on_state_text(int event, const char* name, const char* id) {
    state_cb cb = g_state_cb.load(std::memory_order_acquire);
    if (cb) {
        cb(event, name ? name : "", id ? id : "");
    }
}

// Adapter implementing IAirServerCallback. Video does not travel through
// outputVideo: FgAirplayChannel forwards the undecoded stream via
// bridge_on_h264 instead, which is why this build needs no decoder. Audio does
// come through outputAudio, already decoded to PCM by the library's AAC
// decoder, so it needs no special routing.
class BridgeCallback : public IAirServerCallback {
public:
    void connected(const char* remoteName, const char* remoteDeviceId) override {
        bridge_log("connected: name=%s id=%s", remoteName ? remoteName : "(null)",
             remoteDeviceId ? remoteDeviceId : "(null)");
        bridge_on_state_text(BRIDGE_EVENT_CONNECTED, remoteName, remoteDeviceId);
    }
    void disconnected(const char* remoteName, const char* remoteDeviceId) override {
        bridge_log("disconnected: name=%s id=%s", remoteName ? remoteName : "(null)",
             remoteDeviceId ? remoteDeviceId : "(null)");
        bridge_on_state_text(BRIDGE_EVENT_DISCONNECTED, remoteName, remoteDeviceId);
    }
    void outputAudio(SFgAudioFrame* f, const char*, const char*) override {
        audio_cb cb = g_audio_cb.load(std::memory_order_acquire);
        if (cb && f && f->data && f->dataLen > 0) {
            cb(f->data, (int)f->dataLen, (int)f->sampleRate, (int)f->channels,
               (int)f->bitsPerSample);
        }
    }
    void outputVideo(SFgVideoFrame*, const char*, const char*) override {}
    void videoPlay(char*, double, double) override {}
    void videoGetPlayInfo(double*, double*, double*) override {}
    void setVolume(float volume, const char*, const char*) override {
        // AirPlay volume is dB: 0 = full, -144 = mute. Passed on as text so it
        // rides the existing state callback instead of needing a new export.
        char buf[32];
        snprintf(buf, sizeof(buf), "%.4f", volume);
        bridge_on_state_text(BRIDGE_EVENT_VOLUME, buf, "");
    }
    bool requestPinApproval(const char*, const char*) override { return true; }
    // AirPlayServerLib logs its whole RTSP/RTP/decoder trace through here at
    // debug level. It costs nothing while AIRPLAY_BRIDGE_LOG is unset, and when
    // it is set it is the only view into what a phone actually negotiated —
    // which is how the ALAC requirement was found in the first place.
    void log(int level, const char* msg) override {
        if (msg) {
            bridge_log("[lib:%d] %s", level, msg);
        }
    }
};

static BridgeCallback* g_cb = nullptr;
static void* g_handle = nullptr;

extern "C" {

// Start the receiver. Returns 0 on success, 1 if already running, <0 on error.
// A null `acb` leaves audio off — the receiver then never touches the PCM path.
//
// The name changes whenever the signature does (this was mirror_start_ex before
// audio). A stale DLL and a fresh host then fail loudly at symbol lookup, rather
// than the host quietly pushing an extra argument the DLL reads as garbage.
AIRPLAYSERVER_API int mirror_start_av(const mirror_cfg* cfg, video_cb vcb, state_cb scb,
                                      audio_cb acb) {
    if (!cfg) {
        bridge_log("mirror_start_av: null cfg");
        return MIRROR_ERR_ARGS;
    }
    if (g_handle) {
        bridge_log("mirror_start_av: already running");
        return MIRROR_ALREADY_RUNNING;
    }

    unsigned int raop_port = cfg->raop_port;
    unsigned int airplay_port = cfg->airplay_port;
    if (airplay_port == 0) {
        airplay_port = 7000;
        raop_port = 6999;
    }
    bridge_log("mirror_start_av: name=%s raop=%u airplay=%u",
         cfg->server_name ? cfg->server_name : "(null)", raop_port, airplay_port);

    if (!bonjour_available()) {
        bridge_log("mirror_start_av: dnssd.dll (Bonjour) missing or incomplete");
        return MIRROR_ERR_NO_BONJOUR;
    }
    if (!port_free((unsigned short)airplay_port) || !port_free((unsigned short)raop_port)) {
        bridge_log("mirror_start_av: port %u or %u already in use", airplay_port, raop_port);
        return MIRROR_ERR_PORT_BUSY;
    }

    // Publish the callbacks before the server can raise anything.
    g_video_cb.store(vcb, std::memory_order_release);
    g_state_cb.store(scb, std::memory_order_release);
    g_audio_cb.store(acb, std::memory_order_release);
    bridge_log("mirror_start_av: audio %s", acb ? "enabled" : "disabled");

    g_cb = new BridgeCallback();
    g_handle = fgServerStart(cfg->server_name, raop_port, airplay_port, g_cb, cfg->password);
    if (!g_handle) {
        // Reached when the stack itself failed — Bonjour present but its
        // service not running, a socket refused by policy, and so on.
        bridge_log("mirror_start_av: fgServerStart failed");
        g_video_cb.store(nullptr, std::memory_order_release);
        g_state_cb.store(nullptr, std::memory_order_release);
        g_audio_cb.store(nullptr, std::memory_order_release);
        delete g_cb;
        g_cb = nullptr;
        return MIRROR_ERR_START;
    }
    bridge_log("mirror_start_av: started");
    return MIRROR_OK;
}

// Optional: register before or after mirror_start_av, whenever the host wants
// artwork. Null clears it.
AIRPLAYSERVER_API void mirror_set_art_cb(art_cb cb) {
    g_art_cb.store(cb, std::memory_order_release);
    bridge_log("artwork callback %s", cb ? "registered" : "cleared");
}

AIRPLAYSERVER_API void mirror_stop(void) {
    bridge_log("mirror_stop");
    // Stop delivering to the host first: past this point the host may free
    // whatever the callbacks write into.
    g_video_cb.store(nullptr, std::memory_order_release);
    g_state_cb.store(nullptr, std::memory_order_release);
    g_audio_cb.store(nullptr, std::memory_order_release);
    g_art_cb.store(nullptr, std::memory_order_release);
    // The next session negotiates its own format; do not carry a decoder over.
    bridge_reset_audio_codec();

    if (g_handle) {
        fgServerStop(g_handle);
        g_handle = nullptr;
    }
    if (g_cb) {
        delete g_cb;
        g_cb = nullptr;
    }
    bridge_log("mirror_stop done");
}

} // extern "C"
