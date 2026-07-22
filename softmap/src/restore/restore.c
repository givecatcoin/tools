#include "softmap/restore.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#else
#include <errno.h>
#include <sys/stat.h>
#endif

static int path_is_absolute(const char *p) {
    if (!p || !*p)
        return 0;
    if (p[0] && p[1] == ':')
        return 1;
    return p[0] == '/' || p[0] == '\\';
}

/* Returns 1 if a map rule was applied, 0 otherwise. */
static int apply_map(const sm_path_map_t *maps, int map_count, const char *src,
                     char *dst, size_t dstsz) {
    int best = -1;
    size_t best_len = 0;
    int i;
    for (i = 0; i < map_count; ++i) {
        size_t flen = strlen(maps[i].from);
        if (sm_str_starts_with_ci(src, maps[i].from) && flen > best_len) {
            best = i;
            best_len = flen;
        }
    }
    if (best < 0) {
        snprintf(dst, dstsz, "%s", src);
        return 0;
    }
    snprintf(dst, dstsz, "%s%s", maps[best].to, src + best_len);
    return 1;
}

static void join_under_target(const char *target, const char *mapped,
                              char *out, size_t outsz) {
    /* Place relative path under --target (strip drive/root of mapped). */
    const char *rel = mapped;
    if (path_is_absolute(mapped) && target && *target) {
        if (mapped[1] == ':')
            rel = mapped + 2;
        while (*rel == '\\' || *rel == '/')
            ++rel;
        size_t tlen = strlen(target);
        if (tlen > 0 && (target[tlen - 1] == '\\' || target[tlen - 1] == '/'))
            snprintf(out, outsz, "%s%s", target, rel);
        else
            snprintf(out, outsz, "%s\\%s", target, rel);
#if !defined(_WIN32)
        for (char *p = out; *p; ++p) {
            if (*p == '\\')
                *p = '/';
        }
#endif
        return;
    }
    snprintf(out, outsz, "%s", mapped);
}

static void resolve_restore_path(const char *target, const sm_path_map_t *maps,
                                 int map_count, const char *src, char *out,
                                 size_t outsz) {
    char mapped[SM_MAX_PATH];
    int mapped_hit = apply_map(maps, map_count, src, mapped, sizeof(mapped));
    /*
     * Absolute --map destination is final (do not also join under --target).
     * Otherwise strip drive / join under --target.
     */
    if (mapped_hit && path_is_absolute(mapped)) {
        snprintf(out, outsz, "%s", mapped);
        return;
    }
    join_under_target(target, mapped, out, outsz);
}

static int mkdir_one(const char *path) {
#ifdef _WIN32
    wchar_t wpath[SM_MAX_PATH];
    if (MultiByteToWideChar(CP_UTF8, 0, path, -1, wpath, SM_MAX_PATH) <= 0)
        return -1;
    if (CreateDirectoryW(wpath, NULL))
        return 0;
    DWORD err = GetLastError();
    if (err == ERROR_ALREADY_EXISTS)
        return 0;
    return -1;
#else
    if (mkdir(path, 0755) == 0)
        return 0;
    if (errno == EEXIST)
        return 0;
    return -1;
#endif
}

static int mkdir_p(const char *path) {
    char buf[SM_MAX_PATH];
    strncpy(buf, path, sizeof(buf) - 1);
    buf[sizeof(buf) - 1] = '\0';
#if defined(_WIN32)
    sm_path_normalize_sep(buf);
#endif
    size_t len = strlen(buf);
    if (len == 0)
        return -1;

    size_t i = 0;
#if defined(_WIN32)
    if (buf[0] && buf[1] == ':')
        i = 2;
    if (buf[i] == '\\')
        ++i;
#else
    if (buf[0] == '/')
        i = 1;
#endif
    for (; i < len; ++i) {
#if defined(_WIN32)
        if (buf[i] != '\\')
            continue;
#else
        if (buf[i] != '/')
            continue;
#endif
        char save = buf[i];
        buf[i] = '\0';
        if (mkdir_one(buf) != 0) {
            buf[i] = save;
            /* continue trying */
        }
        buf[i] = save;
    }
    return mkdir_one(path);
}

static int confirm_yes(void) {
    fprintf(stderr, "Create directories? [y/N] ");
    fflush(stderr);
    char line[32];
    if (!fgets(line, sizeof(line), stdin))
        return 0;
    return line[0] == 'y' || line[0] == 'Y';
}

int sm_restore_dirs(const sm_snapshot_t *snap, const char *target,
                    const sm_path_map_t *maps, int map_count, int dry_run,
                    int yes) {
    if (!target || !*target) {
        sm_log(SM_LOG_ERROR, "restore requires --target");
        return 1;
    }

    uint32_t count = 0;
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (n->type == SM_DIR)
            ++count;
    }
    sm_log(SM_LOG_INFO, "%u directories to create under %s%s", count, target,
           dry_run ? " (dry-run)" : "");

    if (!dry_run && !yes && !confirm_yes()) {
        sm_log(SM_LOG_INFO, "cancelled");
        return 0;
    }

    uint32_t ok = 0, fail = 0;
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (n->type != SM_DIR)
            continue;
        char final_path[SM_MAX_PATH];
        resolve_restore_path(target, maps, map_count, n->path, final_path,
                             sizeof(final_path));

        if (dry_run) {
            printf("mkdir %s\n", final_path);
            ++ok;
            continue;
        }
        if (mkdir_p(final_path) == 0)
            ++ok;
        else {
            sm_log(SM_LOG_VERBOSE, "failed: %s", final_path);
            ++fail;
        }
    }
    sm_log(SM_LOG_INFO, "done: ok=%u fail=%u", ok, fail);
    return fail ? 3 : 0;
}
