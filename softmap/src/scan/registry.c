#include "softmap/registry.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

static char *wide_to_utf8(const wchar_t *w) {
    if (!w)
        return sm_strdup("");
    int n = WideCharToMultiByte(CP_UTF8, 0, w, -1, NULL, 0, NULL, NULL);
    if (n <= 0)
        return sm_strdup("");
    char *s = (char *)malloc((size_t)n);
    if (!s)
        return NULL;
    WideCharToMultiByte(CP_UTF8, 0, w, -1, s, n, NULL, NULL);
    return s;
}

static int read_string_value(HKEY key, const wchar_t *name, char **out) {
    DWORD type = 0;
    DWORD size = 0;
    if (RegQueryValueExW(key, name, NULL, &type, NULL, &size) != ERROR_SUCCESS ||
        (type != REG_SZ && type != REG_EXPAND_SZ) || size == 0) {
        *out = sm_strdup("");
        return 0;
    }
    wchar_t *buf = (wchar_t *)malloc(size + sizeof(wchar_t));
    if (!buf)
        return -1;
    if (RegQueryValueExW(key, name, NULL, &type, (LPBYTE)buf, &size) !=
        ERROR_SUCCESS) {
        free(buf);
        *out = sm_strdup("");
        return 0;
    }
    buf[size / sizeof(wchar_t)] = L'\0';
    *out = wide_to_utf8(buf);
    free(buf);
    return 0;
}

static int read_dword_value(HKEY key, const wchar_t *name, DWORD *out) {
    DWORD type = 0;
    DWORD size = sizeof(DWORD);
    DWORD v = 0;
    if (RegQueryValueExW(key, name, NULL, &type, (LPBYTE)&v, &size) !=
            ERROR_SUCCESS ||
        type != REG_DWORD) {
        *out = 0;
        return -1;
    }
    *out = v;
    return 0;
}

static int has_value(HKEY key, const wchar_t *name) {
    return RegQueryValueExW(key, name, NULL, NULL, NULL, NULL) == ERROR_SUCCESS;
}

static int enum_uninstall(HKEY root, REGSAM sam, int scope, sm_snapshot_t *snap) {
    HKEY base = NULL;
    const wchar_t *path =
        L"SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    if (RegOpenKeyExW(root, path, 0, KEY_READ | sam, &base) != ERROR_SUCCESS)
        return 0;

    DWORD index = 0;
    wchar_t subname[256];
    for (;;) {
        DWORD sublen = 256;
        LONG rc = RegEnumKeyExW(base, index++, subname, &sublen, NULL, NULL,
                                NULL, NULL);
        if (rc == ERROR_NO_MORE_ITEMS)
            break;
        if (rc != ERROR_SUCCESS)
            continue;

        HKEY sub = NULL;
        if (RegOpenKeyExW(base, subname, 0, KEY_READ | sam, &sub) !=
            ERROR_SUCCESS)
            continue;

        DWORD syscomp = 0;
        if (read_dword_value(sub, L"SystemComponent", &syscomp) == 0 &&
            syscomp == 1) {
            RegCloseKey(sub);
            continue;
        }
        if (has_value(sub, L"ParentKeyName")) {
            RegCloseKey(sub);
            continue;
        }

        char *display = NULL;
        read_string_value(sub, L"DisplayName", &display);
        if (!display || !*display) {
            free(display);
            RegCloseKey(sub);
            continue;
        }
        if (sm_str_starts_with_ci(display, "KB")) {
            free(display);
            RegCloseKey(sub);
            continue;
        }

        sm_software_t *sw = sm_software_new();
        if (!sw) {
            free(display);
            RegCloseKey(sub);
            continue;
        }
        sw->scope = scope;
        sw->name = display;
        read_string_value(sub, L"DisplayVersion", &sw->version);
        read_string_value(sub, L"InstallLocation", &sw->location);
        read_string_value(sub, L"Publisher", &sw->publisher);
        sw->uninstall_key = wide_to_utf8(subname);
        sm_snapshot_add_software(snap, sw);
        RegCloseKey(sub);
    }
    RegCloseKey(base);
    return 0;
}

int sm_registry_scan(sm_snapshot_t *snap) {
    sm_log(SM_LOG_INFO, "scanning registry (Uninstall)...");
    enum_uninstall(HKEY_LOCAL_MACHINE, 0, 0, snap);
    enum_uninstall(HKEY_LOCAL_MACHINE, KEY_WOW64_32KEY, 0, snap);
    enum_uninstall(HKEY_CURRENT_USER, 0, 1, snap);
    sm_log(SM_LOG_INFO, "found %u software entries", snap->software_count);
    return 0;
}

#else /* !_WIN32 */

int sm_registry_scan(sm_snapshot_t *snap) {
    (void)snap;
    sm_log(SM_LOG_INFO,
           "registry scan skipped (Windows only); BF1 empty on this platform");
    return 0;
}

#endif
