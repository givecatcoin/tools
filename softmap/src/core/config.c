#include "softmap/config.h"
#include "softmap/util.h"

#include <ctype.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#if defined(_WIN32)
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#endif

static char *trim(char *s) {
    while (*s && isspace((unsigned char)*s))
        ++s;
    if (!*s)
        return s;
    char *e = s + strlen(s) - 1;
    while (e > s && isspace((unsigned char)*e))
        *e-- = '\0';
    return s;
}

static void clear_drives(sm_config_t *cfg) {
    int i;
    for (i = 0; i < cfg->drive_count; ++i)
        sm_free(cfg->drives[i]);
    cfg->drive_count = 0;
}

int sm_config_detect_drives(sm_config_t *cfg) {
    clear_drives(cfg);
#if defined(_WIN32)
    {
        DWORD mask = GetLogicalDrives();
        int i;
        char listed[256];
        size_t listed_n = 0;
        listed[0] = '\0';

        for (i = 0; i < 26; ++i) {
            char root[4];
            UINT type;
            if (!(mask & (1u << i)))
                continue;
            root[0] = (char)('A' + i);
            root[1] = ':';
            root[2] = '\\';
            root[3] = '\0';
            type = GetDriveTypeA(root);
            /* Fixed (+ RAM) only. Removable/CD/network skipped by default. */
            if (type != DRIVE_FIXED && type != DRIVE_RAMDISK)
                continue;
            if (sm_config_add_drive(cfg, root) != 0)
                break;
            if (listed_n + 4 < sizeof(listed)) {
                if (listed_n > 0)
                    listed[listed_n++] = ' ';
                listed[listed_n++] = root[0];
                listed[listed_n++] = ':';
                listed[listed_n++] = '\\';
                listed[listed_n] = '\0';
            }
        }
        if (cfg->drive_count == 0) {
            sm_log(SM_LOG_ERROR, "no fixed drives detected");
            return -1;
        }
        sm_log(SM_LOG_INFO, "detected drives: %s", listed);
    }
#else
    sm_config_add_drive(cfg, "/");
    sm_log(SM_LOG_INFO, "detected drives: /");
#endif
    return 0;
}

void sm_config_init_defaults(sm_config_t *cfg) {
    memset(cfg, 0, sizeof(*cfg));
    cfg->depth = SM_DEPTH_ALL_FILES;
    if (sm_config_detect_drives(cfg) != 0)
        cfg->drive_count = 0;
    sm_config_add_exclude(cfg, "Windows");
    sm_config_add_exclude(cfg, "$Recycle.Bin");
    sm_config_add_exclude(cfg, "System Volume Information");
    sm_config_add_exclude(cfg, "PerfLogs");
    sm_config_add_exclude(cfg, "Recovery");
    sm_config_add_exclude(cfg, "Config.Msi");
    sm_config_add_exclude(cfg, "pagefile.sys");
    sm_config_add_exclude(cfg, "hiberfil.sys");
    sm_config_add_exclude(cfg, "node_modules");
    sm_config_add_exclude(cfg, ".git");
}

void sm_config_free(sm_config_t *cfg) {
    int i;
    for (i = 0; i < cfg->drive_count; ++i)
        sm_free(cfg->drives[i]);
    for (i = 0; i < cfg->exclude_count; ++i)
        sm_free(cfg->excludes[i]);
    memset(cfg, 0, sizeof(*cfg));
}

int sm_config_add_drive(sm_config_t *cfg, const char *drive) {
    if (!drive || cfg->drive_count >= SM_MAX_DRIVES)
        return -1;
    char buf[SM_MAX_PATH];
    strncpy(buf, drive, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';
#if defined(_WIN32)
    sm_path_normalize_sep(buf);
    size_t n = strlen(buf);
    if (n > 0 && buf[n - 1] != '\\') {
        if (n + 1 < sizeof(buf)) {
            buf[n] = '\\';
            buf[n + 1] = '\0';
        }
    }
#else
    /* Keep POSIX paths as-is; convert accidental backslashes to '/' */
    for (char *p = buf; *p; ++p) {
        if (*p == '\\')
            *p = '/';
    }
    size_t n = strlen(buf);
    if (n > 1 && buf[n - 1] == '/')
        buf[n - 1] = '\0';
#endif
    cfg->drives[cfg->drive_count++] = sm_strdup(buf);
    return 0;
}

int sm_config_add_exclude(sm_config_t *cfg, const char *name) {
    if (!name || !*name || cfg->exclude_count >= SM_MAX_EXCLUDES)
        return -1;
    cfg->excludes[cfg->exclude_count++] = sm_strdup(name);
    return 0;
}

int sm_config_load(sm_config_t *cfg, const char *path) {
    FILE *fp = fopen(path, "r");
    if (!fp)
        return 1; /* not found / unreadable */

    /* Skip UTF-8 BOM if present (common from Windows editors). */
    {
        int c1 = fgetc(fp);
        if (c1 == 0xEF) {
            int c2 = fgetc(fp);
            int c3 = fgetc(fp);
            if (!(c2 == 0xBB && c3 == 0xBF)) {
                if (c3 != EOF)
                    ungetc(c3, fp);
                if (c2 != EOF)
                    ungetc(c2, fp);
                ungetc(c1, fp);
            }
        } else if (c1 != EOF) {
            ungetc(c1, fp);
        }
    }

    /* drives= replaces; exclude= appends to built-in defaults */
    int saw_drive = 0;
    char line[1024];
    char section[64] = "";

    while (fgets(line, sizeof(line), fp)) {
        char *p = trim(line);
        if (!*p || *p == ';' || *p == '#')
            continue;
        if (*p == '[') {
            char *end = strchr(p, ']');
            if (!end)
                continue;
            *end = '\0';
            strncpy(section, p + 1, sizeof(section) - 1);
            section[sizeof(section) - 1] = '\0';
            continue;
        }
        char *eq = strchr(p, '=');
        if (!eq)
            continue;
        *eq = '\0';
        char *key = trim(p);
        char *val = trim(eq + 1);
        if (strcmp(section, "tree") != 0)
            continue;

        if (strcmp(key, "drives") == 0) {
            if (!saw_drive) {
                clear_drives(cfg);
                saw_drive = 1;
            }
            if (strcmp(val, "auto") == 0 || strcmp(val, "*") == 0) {
                sm_config_detect_drives(cfg);
            } else {
                sm_config_add_drive(cfg, val);
            }
        } else if (strcmp(key, "exclude") == 0) {
            sm_config_add_exclude(cfg, val);
        } else if (strcmp(key, "depth") == 0) {
            if (strcmp(val, "folders_and_apps") == 0)
                cfg->depth = SM_DEPTH_FOLDERS_AND_APPS;
            else
                cfg->depth = SM_DEPTH_ALL_FILES;
        }
    }
    fclose(fp);
    sm_log(SM_LOG_INFO, "loaded config: %s", path);
    return 0;
}
