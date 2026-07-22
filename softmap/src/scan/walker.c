#include "softmap/walker.h"
#include "softmap/filter.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>

/* Extended-length path support (\\?\ ...). */
#define SM_WIDE_MAX 32768

static char *wide_to_utf8(const wchar_t *w) {
    int n = WideCharToMultiByte(CP_UTF8, 0, w, -1, NULL, 0, NULL, NULL);
    if (n <= 0)
        return sm_strdup("");
    char *s = (char *)malloc((size_t)n);
    if (!s)
        return NULL;
    WideCharToMultiByte(CP_UTF8, 0, w, -1, s, n, NULL, NULL);
    return s;
}

static int should_record_file(const sm_config_t *cfg, const char *name) {
    if (cfg->depth == SM_DEPTH_ALL_FILES)
        return 1;
    return sm_path_has_ext(name, ".exe") || sm_path_has_ext(name, ".lnk");
}

/* Build \\?\ prefixed wide path for Win32 APIs. out must hold SM_WIDE_MAX. */
static int to_extended_wide(const char *utf8, wchar_t *out, size_t out_chars) {
    wchar_t raw[SM_WIDE_MAX];
    int n = MultiByteToWideChar(CP_UTF8, 0, utf8, -1, raw, SM_WIDE_MAX);
    if (n <= 0)
        return -1;

    if (wcsncmp(raw, L"\\\\?\\", 4) == 0) {
        if ((size_t)n > out_chars)
            return -1;
        memcpy(out, raw, (size_t)n * sizeof(wchar_t));
        return 0;
    }
    /* UNC: \\server\share -> \\?\UNC\server\share */
    if (raw[0] == L'\\' && raw[1] == L'\\') {
        if (_snwprintf(out, out_chars, L"\\\\?\\UNC\\%s", raw + 2) < 0)
            return -1;
        return 0;
    }
    if (_snwprintf(out, out_chars, L"\\\\?\\%s", raw) < 0)
        return -1;
    return 0;
}

static HANDLE find_first_dir(const char *dir_utf8, WIN32_FIND_DATAW *fd) {
    wchar_t *pattern = (wchar_t *)malloc(SM_WIDE_MAX * sizeof(wchar_t));
    wchar_t *wdir = (wchar_t *)malloc(SM_WIDE_MAX * sizeof(wchar_t));
    HANDLE h = INVALID_HANDLE_VALUE;

    if (!pattern || !wdir)
        goto done;
    if (to_extended_wide(dir_utf8, wdir, SM_WIDE_MAX) != 0)
        goto done;
    if (_snwprintf(pattern, SM_WIDE_MAX, L"%s\\*", wdir) < 0)
        goto done;

    h = FindFirstFileW(pattern, fd);
    if (h == INVALID_HANDLE_VALUE) {
        /* Fallback without prefix (rare; some special volumes). */
        int n = MultiByteToWideChar(CP_UTF8, 0, dir_utf8, -1, wdir, SM_WIDE_MAX);
        if (n > 0 && _snwprintf(pattern, SM_WIDE_MAX, L"%s\\*", wdir) >= 0)
            h = FindFirstFileW(pattern, fd);
    }

done:
    free(pattern);
    free(wdir);
    return h;
}

static int walk_dir(sm_snapshot_t *snap, const sm_config_t *cfg,
                    const char *dir_utf8) {
    char dir_clean[SM_MAX_PATH];

    strncpy(dir_clean, dir_utf8, sizeof(dir_clean) - 1);
    dir_clean[sizeof(dir_clean) - 1] = '\0';
    sm_path_normalize_sep(dir_clean);
    size_t dlen = strlen(dir_clean);
    while (dlen > 3 && dir_clean[dlen - 1] == '\\') {
        dir_clean[--dlen] = '\0';
    }

    WIN32_FIND_DATAW fd;
    HANDLE h = find_first_dir(dir_clean, &fd);
    if (h == INVALID_HANDLE_VALUE) {
        DWORD err = GetLastError();
        if (err == ERROR_ACCESS_DENIED || err == ERROR_PATH_NOT_FOUND ||
            err == ERROR_FILENAME_EXCED_RANGE) {
            sm_log(SM_LOG_INFO, "skip (%lu): %s", (unsigned long)err, dir_clean);
        } else {
            sm_log(SM_LOG_VERBOSE, "skip (%lu): %s", (unsigned long)err,
                   dir_clean);
        }
        return 0;
    }

    do {
        if (wcscmp(fd.cFileName, L".") == 0 || wcscmp(fd.cFileName, L"..") == 0)
            continue;

        char *name = wide_to_utf8(fd.cFileName);
        if (!name)
            continue;

        char child[SM_MAX_PATH];
        int n = snprintf(child, sizeof(child), "%s\\%s", dir_clean, name);
        if (n < 0 || (size_t)n >= sizeof(child)) {
            sm_log(SM_LOG_INFO, "skip (path too long): %s\\%s", dir_clean, name);
            free(name);
            continue;
        }

        if (sm_should_exclude(cfg, child, name)) {
            free(name);
            continue;
        }

        int is_dir = (fd.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) != 0;
        int is_reparse =
            (fd.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT) != 0;

        if (is_dir) {
            sm_snapshot_add_node(snap, SM_DIR, child, 0, 0);
            if (!is_reparse)
                walk_dir(snap, cfg, child);
        } else if (should_record_file(cfg, name)) {
            ULARGE_INTEGER sz;
            sz.LowPart = fd.nFileSizeLow;
            sz.HighPart = fd.nFileSizeHigh;
            sm_node_type_t t =
                sm_path_has_ext(name, ".lnk") ? SM_LINK : SM_FILE;
            sm_snapshot_add_node(snap, t, child, (uint64_t)sz.QuadPart, 0);
        }
        free(name);
    } while (FindNextFileW(h, &fd));

    FindClose(h);
    return 0;
}

int sm_walk_drives(sm_snapshot_t *snap, const sm_config_t *cfg) {
    int i;
    for (i = 0; i < cfg->drive_count; ++i) {
        const char *root = cfg->drives[i];
        char root_clean[SM_MAX_PATH];
        strncpy(root_clean, root, sizeof(root_clean) - 1);
        root_clean[sizeof(root_clean) - 1] = '\0';
        sm_path_normalize_sep(root_clean);
        size_t n = strlen(root_clean);
        while (n > 3 && root_clean[n - 1] == '\\')
            root_clean[--n] = '\0';

        sm_log(SM_LOG_INFO, "walking %s ...", root_clean);
        sm_snapshot_add_node(snap, SM_DIR, root_clean, 0, 0);
        walk_dir(snap, cfg, root_clean);
    }
    sm_log(SM_LOG_INFO, "tree nodes: %llu (dirs=%u files=%u)",
           (unsigned long long)snap->node_count, snap->dir_count,
           snap->file_count);
    return 0;
}

#else /* POSIX walker for development / non-Windows */

#include <dirent.h>
#include <sys/stat.h>
#include <unistd.h>

static int should_record_file(const sm_config_t *cfg, const char *name) {
    if (cfg->depth == SM_DEPTH_ALL_FILES)
        return 1;
    return sm_path_has_ext(name, ".exe") || sm_path_has_ext(name, ".lnk");
}

static void join_path(char *out, size_t outsz, const char *dir, const char *name) {
    size_t dlen = strlen(dir);
    if (dlen > 0 && dir[dlen - 1] == '/')
        snprintf(out, outsz, "%s%s", dir, name);
    else
        snprintf(out, outsz, "%s/%s", dir, name);
}

static int walk_dir(sm_snapshot_t *snap, const sm_config_t *cfg, const char *dir) {
    DIR *d = opendir(dir);
    if (!d) {
        sm_log(SM_LOG_INFO, "skip (access): %s", dir);
        return 0;
    }
    struct dirent *ent;
    while ((ent = readdir(d)) != NULL) {
        if (strcmp(ent->d_name, ".") == 0 || strcmp(ent->d_name, "..") == 0)
            continue;
        char child[SM_MAX_PATH];
        join_path(child, sizeof(child), dir, ent->d_name);
        if (sm_should_exclude(cfg, child, ent->d_name))
            continue;

        struct stat st;
        if (lstat(child, &st) != 0)
            continue;
        if (S_ISDIR(st.st_mode)) {
            sm_snapshot_add_node(snap, SM_DIR, child, 0, 0);
            if (!S_ISLNK(st.st_mode))
                walk_dir(snap, cfg, child);
        } else if (S_ISREG(st.st_mode) &&
                   should_record_file(cfg, ent->d_name)) {
            sm_node_type_t t =
                sm_path_has_ext(ent->d_name, ".lnk") ? SM_LINK : SM_FILE;
            sm_snapshot_add_node(snap, t, child, (uint64_t)st.st_size,
                                 (int64_t)st.st_mtime);
        }
    }
    closedir(d);
    return 0;
}

int sm_walk_drives(sm_snapshot_t *snap, const sm_config_t *cfg) {
    int i;
    for (i = 0; i < cfg->drive_count; ++i) {
        const char *root = cfg->drives[i];
        /* On POSIX, treat "C:\" style as skip unless path exists; allow real paths */
        if (strlen(root) >= 2 && root[1] == ':') {
            sm_log(SM_LOG_VERBOSE, "skip Windows drive path on POSIX: %s", root);
            continue;
        }
        sm_log(SM_LOG_INFO, "walking %s ...", root);
        sm_snapshot_add_node(snap, SM_DIR, root, 0, 0);
        walk_dir(snap, cfg, root);
    }
    sm_log(SM_LOG_INFO, "tree nodes: %llu (dirs=%u files=%u)",
           (unsigned long long)snap->node_count, snap->dir_count,
           snap->file_count);
    return 0;
}

#endif
