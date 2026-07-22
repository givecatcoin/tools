#include "softmap/filter.h"
#include "softmap/util.h"

#include <string.h>

static int name_matches_exclude(const char *name, const char *ex) {
    return sm_strcasecmp_ascii(name, ex) == 0;
}

/* Exclude if any path component matches an exclude name (case-insensitive). */
int sm_should_exclude(const sm_config_t *cfg, const char *path, const char *name) {
    int i;
    if (name && *name) {
        for (i = 0; i < cfg->exclude_count; ++i) {
            if (name_matches_exclude(name, cfg->excludes[i]))
                return 1;
        }
    }
    if (!path)
        return 0;

    /* Walk path components */
    char buf[SM_MAX_PATH];
    strncpy(buf, path, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';
    sm_path_normalize_sep(buf);

    char *p = buf;
    /* Skip drive prefix like C:\ */
    if (p[0] && p[1] == ':' && (p[2] == '\\' || p[2] == '\0'))
        p += (p[2] == '\\') ? 3 : 2;

    while (*p) {
        char *sep = strchr(p, '\\');
        if (sep)
            *sep = '\0';
        if (*p) {
            for (i = 0; i < cfg->exclude_count; ++i) {
                if (name_matches_exclude(p, cfg->excludes[i]))
                    return 1;
            }
        }
        if (!sep)
            break;
        p = sep + 1;
    }
    return 0;
}
