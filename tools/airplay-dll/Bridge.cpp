// Bridge.cpp : C ABI bridge between AirPlayServerLib and the Rust/Tauri host.
//
//   typedef void (*video_cb)(const uint8_t* data, int len, int frame_type);
//   typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);
//
//   int  mirror_start_ex(const mirror_cfg* cfg, video_cb vcb, state_cb scb);
//   void mirror_stop(void);
//
// The host receives the H.264 elementary stream as it arrives and decodes it
// itself, so nothing here touches FFmpeg and no named pipe is involved.
// `state_cb` surfaces the receiver's connect/disconnect edges, which the
// protocol stack already tracks: AirPlayServerLib raises them from
// raop_rtp_start_mirror / raop_rtp_mirror_stop, so a disconnect is reported on
// both an RTSP TEARDOWN and an abrupt socket close.
//
// Both callbacks run on AirPlayServerLib's network threads and must return
// promptly. `data` is owned by the caller and is freed as soon as video_cb
// returns — copy anything you need to keep.

#include "Airplay2Head.h"
#include "BridgeTap.h"

#include <windows.h>
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

#define BRIDGE_EVENT_CONNECTED    0
#define BRIDGE_EVENT_DISCONNECTED 1

// ── opt-in diagnostics ───────────────────────────────────────────────────
// Off unless AIRPLAY_BRIDGE_LOG names a file. Lifecycle events only: an
// earlier build of this DLL logged every frame through a fopen/fclose pair on
// a path hardcoded to one developer's home directory.

static FILE* g_logf = nullptr;
static std::once_flag g_log_once;
static std::mutex g_log_mutex;

static void blog(const char* fmt, ...) {
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

void bridge_on_h264(const unsigned char* data, int len, int is_codec_config) {
    video_cb cb = g_video_cb.load(std::memory_order_acquire);
    if (cb && data && len > 0) {
        cb((const uint8_t*)data, len, is_codec_config ? 0 : 1);
    }
}

static void bridge_on_state(int event, const char* name, const char* id) {
    state_cb cb = g_state_cb.load(std::memory_order_acquire);
    if (cb) {
        cb(event, name ? name : "", id ? id : "");
    }
}

// Adapter implementing IAirServerCallback. Video does not travel through
// outputVideo: FgAirplayChannel forwards the undecoded stream via
// bridge_on_h264 instead, which is why this build needs no decoder.
class BridgeCallback : public IAirServerCallback {
public:
    void connected(const char* remoteName, const char* remoteDeviceId) override {
        blog("connected: name=%s id=%s", remoteName ? remoteName : "(null)",
             remoteDeviceId ? remoteDeviceId : "(null)");
        bridge_on_state(BRIDGE_EVENT_CONNECTED, remoteName, remoteDeviceId);
    }
    void disconnected(const char* remoteName, const char* remoteDeviceId) override {
        blog("disconnected: name=%s id=%s", remoteName ? remoteName : "(null)",
             remoteDeviceId ? remoteDeviceId : "(null)");
        bridge_on_state(BRIDGE_EVENT_DISCONNECTED, remoteName, remoteDeviceId);
    }
    void outputAudio(SFgAudioFrame*, const char*, const char*) override {}
    void outputVideo(SFgVideoFrame*, const char*, const char*) override {}
    void videoPlay(char*, double, double) override {}
    void videoGetPlayInfo(double*, double*, double*) override {}
    void setVolume(float, const char*, const char*) override {}
    bool requestPinApproval(const char*, const char*) override { return true; }
    void log(int, const char*) override {}
};

static BridgeCallback* g_cb = nullptr;
static void* g_handle = nullptr;

extern "C" {

// Start the receiver. Returns 0 on success, 1 if already running, <0 on error.
AIRPLAYSERVER_API int mirror_start_ex(const mirror_cfg* cfg, video_cb vcb, state_cb scb) {
    if (!cfg) {
        blog("mirror_start_ex: null cfg");
        return -1;
    }
    if (g_handle) {
        blog("mirror_start_ex: already running");
        return 1;
    }

    unsigned int raop_port = cfg->raop_port;
    unsigned int airplay_port = cfg->airplay_port;
    if (airplay_port == 0) {
        airplay_port = 7000;
        raop_port = 6999;
    }
    blog("mirror_start_ex: name=%s raop=%u airplay=%u",
         cfg->server_name ? cfg->server_name : "(null)", raop_port, airplay_port);

    // Publish the callbacks before the server can raise anything.
    g_video_cb.store(vcb, std::memory_order_release);
    g_state_cb.store(scb, std::memory_order_release);

    g_cb = new BridgeCallback();
    g_handle = fgServerStart(cfg->server_name, raop_port, airplay_port, g_cb, cfg->password);
    if (!g_handle) {
        blog("mirror_start_ex: fgServerStart returned NULL");
        g_video_cb.store(nullptr, std::memory_order_release);
        g_state_cb.store(nullptr, std::memory_order_release);
        delete g_cb;
        g_cb = nullptr;
        return -2;
    }
    blog("mirror_start_ex: started");
    return 0;
}

AIRPLAYSERVER_API void mirror_stop(void) {
    blog("mirror_stop");
    // Stop delivering to the host first: past this point the host may free
    // whatever the callbacks write into.
    g_video_cb.store(nullptr, std::memory_order_release);
    g_state_cb.store(nullptr, std::memory_order_release);

    if (g_handle) {
        fgServerStop(g_handle);
        g_handle = nullptr;
    }
    if (g_cb) {
        delete g_cb;
        g_cb = nullptr;
    }
    blog("mirror_stop done");
}

} // extern "C"
