#include "softmap/util.h"

#include <ctype.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

static sm_log_level_t g_log_level = SM_LOG_INFO;

void sm_set_log_level(sm_log_level_t level) {
    g_log_level = level;
}

void sm_log(sm_log_level_t level, const char *fmt, ...) {
    if (level > g_log_level)
        return;
    FILE *out = (level == SM_LOG_ERROR) ? stderr : stderr;
    const char *tag = (level == SM_LOG_ERROR) ? "error" :
                      (level == SM_LOG_VERBOSE) ? "debug" : "info";
    fprintf(out, "softmap: %s: ", tag);
    va_list ap;
    va_start(ap, fmt);
    vfprintf(out, fmt, ap);
    va_end(ap);
    fputc('\n', out);
}

char *sm_strdup(const char *s) {
    if (!s)
        return NULL;
    size_t n = strlen(s);
    char *d = (char *)malloc(n + 1);
    if (!d)
        return NULL;
    memcpy(d, s, n + 1);
    return d;
}

char *sm_strndup(const char *s, size_t n) {
    if (!s)
        return NULL;
    size_t len = strlen(s);
    if (n < len)
        len = n;
    char *d = (char *)malloc(len + 1);
    if (!d)
        return NULL;
    memcpy(d, s, len);
    d[len] = '\0';
    return d;
}

void sm_free(void *p) {
    free(p);
}

int64_t sm_now_unix(void) {
    return (int64_t)time(NULL);
}

void sm_format_time(int64_t t, char *buf, size_t buflen) {
    time_t tt = (time_t)t;
    struct tm tm_buf;
#if defined(_WIN32)
    localtime_s(&tm_buf, &tt);
#else
    localtime_r(&tt, &tm_buf);
#endif
    strftime(buf, buflen, "%Y-%m-%d %H:%M", &tm_buf);
}

int sm_parse_time(const char *s, int64_t *out) {
    int y, mo, d, h, mi;
    struct tm tm_buf;
    if (!s || !out)
        return -1;
    if (sscanf(s, "%d-%d-%d %d:%d", &y, &mo, &d, &h, &mi) != 5)
        return -1;
    memset(&tm_buf, 0, sizeof(tm_buf));
    tm_buf.tm_year = y - 1900;
    tm_buf.tm_mon = mo - 1;
    tm_buf.tm_mday = d;
    tm_buf.tm_hour = h;
    tm_buf.tm_min = mi;
    tm_buf.tm_isdst = -1;
    {
        time_t t = mktime(&tm_buf);
        if (t == (time_t)-1)
            return -1;
        *out = (int64_t)t;
    }
    return 0;
}

const char *sm_basename(const char *path) {
    if (!path || !*path)
        return "";
    const char *slash = strrchr(path, '/');
    const char *bslash = strrchr(path, '\\');
    const char *p = path;
    if (slash && (!bslash || slash > bslash))
        p = slash + 1;
    else if (bslash)
        p = bslash + 1;
    return p;
}

int sm_path_has_ext(const char *path, const char *ext) {
    if (!path || !ext)
        return 0;
    size_t plen = strlen(path);
    size_t elen = strlen(ext);
    if (plen < elen)
        return 0;
    return sm_strcasecmp_ascii(path + plen - elen, ext) == 0;
}

void sm_path_normalize_sep(char *path) {
    if (!path)
        return;
    for (; *path; ++path) {
        if (*path == '/')
            *path = '\\';
    }
}

int sm_strcasecmp_ascii(const char *a, const char *b) {
    if (!a || !b)
        return (a == b) ? 0 : (a ? 1 : -1);
    while (*a && *b) {
        int ca = (unsigned char)tolower((unsigned char)*a);
        int cb = (unsigned char)tolower((unsigned char)*b);
        if (ca != cb)
            return ca - cb;
        ++a;
        ++b;
    }
    return (unsigned char)tolower((unsigned char)*a) -
           (unsigned char)tolower((unsigned char)*b);
}

int sm_str_starts_with_ci(const char *s, const char *prefix) {
    if (!s || !prefix)
        return 0;
    while (*prefix) {
        int cs = (unsigned char)tolower((unsigned char)*s);
        int cp = (unsigned char)tolower((unsigned char)*prefix);
        if (!*s || cs != cp)
            return 0;
        ++s;
        ++prefix;
    }
    return 1;
}
