// Smoke test for a freshly built airplay2dll.dll.
//
// Loads the DLL by absolute path, starts the receiver, lets it advertise for a
// few seconds, then stops it. Checks three things that have bitten this project
// before: that the DLL loads at all, that mirror_start_av returns promptly
// instead of hanging, and that both survive a load path containing a space.
//
//   gcc -O2 -o smoke_test.exe smoke_test.c
//   smoke_test.exe "C:\some dir\airplay2dll.dll" [airplay_port]
//
// A phone is not required: without one, zero callbacks is the expected result.

#include <windows.h>
#include <stdio.h>

typedef struct mirror_cfg {
    const char* server_name;
    unsigned int raop_port;
    unsigned int airplay_port;
    const char* password;
    int width;
    int height;
    int fps;
} mirror_cfg;

typedef void (*video_cb)(const unsigned char* data, int len, int frame_type);
typedef void (*state_cb)(int event, const char* remote_name, const char* device_id);
typedef void (*audio_cb)(const unsigned char* pcm, int len, int sample_rate, int channels,
                         int bits_per_sample);
typedef int  (*mirror_start_av_t)(const mirror_cfg*, video_cb, state_cb, audio_cb);
typedef void (*mirror_stop_t)(void);

static volatile LONG g_video_calls = 0;
static volatile LONG g_video_bytes = 0;
static volatile LONG g_state_calls = 0;
static volatile LONG g_audio_calls = 0;

static void on_video(const unsigned char* data, int len, int frame_type) {
    InterlockedIncrement(&g_video_calls);
    InterlockedExchangeAdd(&g_video_bytes, len);
    if (g_video_calls <= 3) {
        printf("  [video] #%ld frame_type=%d len=%d\n", g_video_calls, frame_type, len);
        fflush(stdout);
    }
}

static void on_audio(const unsigned char* pcm, int len, int sample_rate, int channels,
                     int bits_per_sample) {
    (void)pcm;
    InterlockedIncrement(&g_audio_calls);
    if (g_audio_calls <= 3) {
        printf("  [audio] #%ld %d bytes %dHz %dch %dbit\n",
               g_audio_calls, len, sample_rate, channels, bits_per_sample);
        fflush(stdout);
    }
}

static void on_state(int event, const char* name, const char* id) {
    InterlockedIncrement(&g_state_calls);
    printf("  [state] %s name=%s id=%s\n",
           event == 0 ? "CONNECTED" : "DISCONNECTED", name, id);
    fflush(stdout);
}

int main(int argc, char** argv) {
    const char* dll_path = argc > 1 ? argv[1] : "airplay2dll.dll";
    unsigned int airplay_port = argc > 2 ? (unsigned int)atoi(argv[2]) : 7010;
    DWORD t0, ms;

    printf("dll:  %s\n", dll_path);
    printf("port: airplay=%u raop=%u\n", airplay_port, airplay_port - 1);

    char dir[MAX_PATH];
    lstrcpynA(dir, dll_path, MAX_PATH);
    char* slash = strrchr(dir, '\\');
    if (!slash) slash = strrchr(dir, '/');
    if (slash) {
        *slash = 0;
        SetDllDirectoryA(dir);
    }

    t0 = GetTickCount();
    HMODULE h = LoadLibraryA(dll_path);
    ms = GetTickCount() - t0;
    if (!h) {
        printf("FAIL: LoadLibrary error %lu\n", GetLastError());
        return 1;
    }
    printf("PASS: loaded in %lu ms\n", ms);

    mirror_start_av_t start = (mirror_start_av_t)GetProcAddress(h, "mirror_start_av");
    mirror_stop_t stop = (mirror_stop_t)GetProcAddress(h, "mirror_stop");
    if (!start || !stop) {
        printf("FAIL: missing exports (start=%p stop=%p)\n", (void*)start, (void*)stop);
        return 1;
    }
    printf("PASS: exports resolved\n");

    mirror_cfg cfg;
    memset(&cfg, 0, sizeof(cfg));
    cfg.server_name = "Smoke Test";
    cfg.airplay_port = airplay_port;
    cfg.raop_port = airplay_port - 1;
    cfg.password = NULL;

    int wait_s = argc > 3 ? atoi(argv[3]) : 5;

    // Two cycles: the DLL keeps global server state and never gets unloaded, so
    // a second start in the same process is the case that has broken before.
    for (int cycle = 1; cycle <= 2; cycle++) {
        printf("\n--- cycle %d ---\n", cycle);

        t0 = GetTickCount();
        int rc = start(&cfg, on_video, on_state, on_audio);
        ms = GetTickCount() - t0;
        printf("mirror_start_av -> rc=%d in %lu ms\n", rc, ms);
        if (rc != 0) {
            printf("FAIL: start returned %d\n", rc);
            return 1;
        }
        if (ms > 5000) {
            printf("FAIL: start took %lu ms (hang?)\n", ms);
            return 1;
        }
        printf("PASS: started without hanging\n");

        printf("advertising for %ds (mirror from a phone now to exercise callbacks)...\n", wait_s);
        fflush(stdout);
        Sleep(wait_s * 1000);

        t0 = GetTickCount();
        stop();
        ms = GetTickCount() - t0;
        printf("mirror_stop -> %lu ms\n", ms);
        printf("PASS: stopped cleanly\n");
    }

    printf("\ncallbacks: video=%ld (%ld bytes) state=%ld audio=%ld\n",
           g_video_calls, g_video_bytes, g_state_calls, g_audio_calls);
    printf("PASS: survived start/stop/start/stop\n");
    return 0;
}
