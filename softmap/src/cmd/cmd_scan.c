#include "softmap/cmd.h"
#include "softmap/config.h"
#include "softmap/registry.h"
#include "softmap/walker.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <string.h>
#include <time.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
static void get_hostname(char *buf, size_t buflen) {
    wchar_t w[64];
    DWORD n = 64;
    if (GetComputerNameW(w, &n)) {
        WideCharToMultiByte(CP_UTF8, 0, w, -1, buf, (int)buflen, NULL, NULL);
    } else {
        snprintf(buf, buflen, "unknown");
    }
}

/* Prefer softmap.conf next to the executable, then cwd. */
static int find_default_config(char *out, size_t outsz) {
    wchar_t wexe[MAX_PATH];
    wchar_t wconf[MAX_PATH];
    DWORD n = GetModuleFileNameW(NULL, wexe, MAX_PATH);
    if (n > 0 && n < MAX_PATH) {
        wchar_t *slash = wcsrchr(wexe, L'\\');
        if (slash) {
            slash[1] = L'\0';
            if (_snwprintf(wconf, MAX_PATH, L"%ssoftmap.conf", wexe) >= 0 &&
                GetFileAttributesW(wconf) != INVALID_FILE_ATTRIBUTES) {
                if (WideCharToMultiByte(CP_UTF8, 0, wconf, -1, out, (int)outsz,
                                        NULL, NULL) > 0)
                    return 0;
            }
        }
    }
    if (GetFileAttributesA("softmap.conf") != INVALID_FILE_ATTRIBUTES) {
        snprintf(out, outsz, "softmap.conf");
        return 0;
    }
    return -1;
}
#else
#include <unistd.h>
static void get_hostname(char *buf, size_t buflen) {
    if (gethostname(buf, buflen) != 0)
        snprintf(buf, buflen, "unknown");
    buf[buflen - 1] = '\0';
}

static int find_default_config(char *out, size_t outsz) {
    if (access("softmap.conf", R_OK) == 0) {
        snprintf(out, outsz, "softmap.conf");
        return 0;
    }
    return -1;
}
#endif

static void default_output_name(char *buf, size_t buflen) {
    time_t t = time(NULL);
    struct tm tm_buf;
#if defined(_WIN32)
    localtime_s(&tm_buf, &t);
#else
    localtime_r(&t, &tm_buf);
#endif
    strftime(buf, buflen, "snapshot-%Y%m%d-%H%M%S.smb", &tm_buf);
}

int cmd_scan(int argc, char **argv) {
    const char *out_path = NULL;
    const char *config_path = NULL;
    int config_explicit = 0;
    int software_only = 0;
    int light = 0;
    char *extra_drives[SM_MAX_DRIVES];
    int extra_drive_count = 0;
    int i;

    for (i = 0; i < argc; ++i) {
        if ((strcmp(argv[i], "-o") == 0 || strcmp(argv[i], "--output") == 0) &&
            i + 1 < argc) {
            out_path = argv[++i];
        } else if ((strcmp(argv[i], "-c") == 0 ||
                    strcmp(argv[i], "--config") == 0) &&
                   i + 1 < argc) {
            config_path = argv[++i];
            config_explicit = 1;
        } else if (strcmp(argv[i], "--software-only") == 0) {
            software_only = 1;
        } else if (strcmp(argv[i], "--light") == 0) {
            light = 1;
        } else if (strcmp(argv[i], "--drive") == 0 && i + 1 < argc) {
            if (extra_drive_count < SM_MAX_DRIVES)
                extra_drives[extra_drive_count++] = argv[++i];
        } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
            printf("Usage: softmap scan [-o file] [-c conf] [--software-only] "
                   "[--light] [--drive X:\\]\n");
            return 0;
        } else {
            sm_log(SM_LOG_ERROR, "unknown scan option: %s", argv[i]);
            return 1;
        }
    }

    char auto_name[128];
    if (!out_path) {
        default_output_name(auto_name, sizeof(auto_name));
        out_path = auto_name;
        sm_log(SM_LOG_INFO, "output: %s", out_path);
    }

    char conf_buf[SM_MAX_PATH];
    if (!config_path) {
        if (find_default_config(conf_buf, sizeof(conf_buf)) == 0)
            config_path = conf_buf;
    }

    sm_config_t cfg;
    sm_config_init_defaults(&cfg);
    if (config_path) {
        int lc = sm_config_load(&cfg, config_path);
        if (lc != 0 && config_explicit) {
            sm_log(SM_LOG_ERROR, "cannot read config: %s", config_path);
            sm_config_free(&cfg);
            return 1;
        }
    }
    if (light)
        cfg.depth = SM_DEPTH_FOLDERS_AND_APPS;
    if (software_only)
        cfg.software_only = 1;
    if (extra_drive_count > 0) {
        int d;
        for (d = 0; d < cfg.drive_count; ++d)
            sm_free(cfg.drives[d]);
        cfg.drive_count = 0;
        for (d = 0; d < extra_drive_count; ++d)
            sm_config_add_drive(&cfg, extra_drives[d]);
    }

    if (!cfg.software_only && cfg.drive_count <= 0) {
        sm_log(SM_LOG_ERROR, "no drives to scan (use --drive or check detection)");
        sm_config_free(&cfg);
        return 1;
    }

    sm_snapshot_t snap;
    sm_snapshot_init(&snap);
    snap.scan_time = sm_now_unix();
    snap.depth_mode = cfg.depth;
    char host[64];
    get_hostname(host, sizeof(host));
    snap.hostname = sm_strdup(host);

    if (sm_registry_scan(&snap) != 0) {
        sm_snapshot_free(&snap);
        sm_config_free(&cfg);
        return 1;
    }

    if (!cfg.software_only) {
        if (sm_walk_drives(&snap, &cfg) != 0) {
            sm_snapshot_free(&snap);
            sm_config_free(&cfg);
            return 1;
        }
    }

    if (sm_snapshot_save(&snap, out_path) != 0) {
        sm_snapshot_free(&snap);
        sm_config_free(&cfg);
        return 1;
    }

    sm_log(SM_LOG_INFO, "saved %s (software=%u nodes=%llu)", out_path,
           snap.software_count, (unsigned long long)snap.node_count);
    sm_snapshot_free(&snap);
    sm_config_free(&cfg);
    return 0;
}
