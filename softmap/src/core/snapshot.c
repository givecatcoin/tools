#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---- little-endian helpers ---- */

static int write_u8(FILE *fp, uint8_t v) {
    return fwrite(&v, 1, 1, fp) == 1 ? 0 : -1;
}
static int write_u16(FILE *fp, uint16_t v) {
    uint8_t b[2] = {(uint8_t)(v & 0xff), (uint8_t)((v >> 8) & 0xff)};
    return fwrite(b, 1, 2, fp) == 2 ? 0 : -1;
}
static int write_u32(FILE *fp, uint32_t v) {
    uint8_t b[4] = {
        (uint8_t)(v), (uint8_t)(v >> 8), (uint8_t)(v >> 16), (uint8_t)(v >> 24)};
    return fwrite(b, 1, 4, fp) == 4 ? 0 : -1;
}
static int write_u64(FILE *fp, uint64_t v) {
    uint8_t b[8];
    int i;
    for (i = 0; i < 8; ++i)
        b[i] = (uint8_t)((v >> (8 * i)) & 0xff);
    return fwrite(b, 1, 8, fp) == 8 ? 0 : -1;
}
static int write_i64(FILE *fp, int64_t v) {
    return write_u64(fp, (uint64_t)v);
}

static int write_str(FILE *fp, const char *s) {
    uint16_t len = 0;
    if (s) {
        size_t n = strlen(s);
        if (n > 65535)
            n = 65535;
        len = (uint16_t)n;
    }
    if (write_u16(fp, len) != 0)
        return -1;
    if (len > 0 && fwrite(s, 1, len, fp) != len)
        return -1;
    return 0;
}

static int read_u8(FILE *fp, uint8_t *v) {
    return fread(v, 1, 1, fp) == 1 ? 0 : -1;
}
static int read_u16(FILE *fp, uint16_t *v) {
    uint8_t b[2];
    if (fread(b, 1, 2, fp) != 2)
        return -1;
    *v = (uint16_t)(b[0] | (b[1] << 8));
    return 0;
}
static int read_u32(FILE *fp, uint32_t *v) {
    uint8_t b[4];
    if (fread(b, 1, 4, fp) != 4)
        return -1;
    *v = (uint32_t)b[0] | ((uint32_t)b[1] << 8) | ((uint32_t)b[2] << 16) |
         ((uint32_t)b[3] << 24);
    return 0;
}
static int read_u64(FILE *fp, uint64_t *v) {
    uint8_t b[8];
    int i;
    if (fread(b, 1, 8, fp) != 8)
        return -1;
    *v = 0;
    for (i = 0; i < 8; ++i)
        *v |= ((uint64_t)b[i]) << (8 * i);
    return 0;
}
static int read_i64(FILE *fp, int64_t *v) {
    uint64_t u;
    if (read_u64(fp, &u) != 0)
        return -1;
    *v = (int64_t)u;
    return 0;
}

static char *read_str(FILE *fp) {
    uint16_t len = 0;
    if (read_u16(fp, &len) != 0)
        return NULL;
    char *s = (char *)malloc((size_t)len + 1);
    if (!s)
        return NULL;
    if (len > 0 && fread(s, 1, len, fp) != len) {
        free(s);
        return NULL;
    }
    s[len] = '\0';
    return s;
}

static int ends_with_ci(const char *path, const char *suf) {
    size_t plen = strlen(path);
    size_t slen = strlen(suf);
    if (plen < slen)
        return 0;
    return sm_strcasecmp_ascii(path + plen - slen, suf) == 0;
}

/* ---- text format ---- */

int sm_snapshot_save_smap(const sm_snapshot_t *snap, const char *path) {
    FILE *fp = fopen(path, "wb");
    if (!fp) {
        sm_log(SM_LOG_ERROR, "cannot write %s", path);
        return -1;
    }
    char tbuf[64];
    sm_format_time(snap->scan_time, tbuf, sizeof(tbuf));
    fprintf(fp, "# SoftMap v1\n");
    fprintf(fp, "# scan: %s\n", tbuf);
    fprintf(fp, "# host: %s\n", snap->hostname ? snap->hostname : "");
    fprintf(fp, "# software: %u\n", snap->software_count);
    fprintf(fp, "# nodes: %llu\n", (unsigned long long)snap->node_count);
    fprintf(fp, "# depth: %s\n",
            snap->depth_mode == SM_DEPTH_FOLDERS_AND_APPS ? "folders_and_apps"
                                                         : "all_files");
    fprintf(fp, "\n[software]\n");
    for (const sm_software_t *sw = snap->software; sw; sw = sw->next) {
        fprintf(fp, "%s\t%s\t%s\t%s\t%s\n", sw->scope == 1 ? "HKCU" : "HKLM",
                sw->name ? sw->name : "", sw->version ? sw->version : "",
                sw->location ? sw->location : "",
                sw->publisher ? sw->publisher : "");
    }
    fprintf(fp, "\n[tree]\n");
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        char t = (n->type == SM_DIR) ? 'D' : (n->type == SM_LINK) ? 'L' : 'F';
        fprintf(fp, "%c\t%s\n", t, n->path ? n->path : "");
    }
    fclose(fp);
    return 0;
}

static char *tab_field(char **pp) {
    char *p = *pp;
    if (!p || !*p)
        return NULL;
    char *tab = strchr(p, '\t');
    if (tab) {
        *tab = '\0';
        *pp = tab + 1;
    } else {
        *pp = p + strlen(p);
    }
    return p;
}

int sm_snapshot_load_smap(sm_snapshot_t *snap, const char *path) {
    FILE *fp = fopen(path, "rb");
    if (!fp) {
        sm_log(SM_LOG_ERROR, "cannot read %s", path);
        return -1;
    }
    sm_snapshot_init(snap);
    char line[SM_MAX_PATH + 128];
    int section = 0; /* 0 none, 1 software, 2 tree */
    int have_scan = 0;

    while (fgets(line, sizeof(line), fp)) {
        size_t len = strlen(line);
        while (len > 0 && (line[len - 1] == '\n' || line[len - 1] == '\r'))
            line[--len] = '\0';
        if (!line[0] || line[0] == '#') {
            if (strncmp(line, "# host: ", 8) == 0) {
                sm_free(snap->hostname);
                snap->hostname = sm_strdup(line + 8);
            } else if (strncmp(line, "# scan: ", 8) == 0) {
                int64_t t = 0;
                if (sm_parse_time(line + 8, &t) == 0) {
                    snap->scan_time = t;
                    have_scan = 1;
                }
            } else if (strncmp(line, "# depth: ", 9) == 0) {
                if (strcmp(line + 9, "folders_and_apps") == 0)
                    snap->depth_mode = SM_DEPTH_FOLDERS_AND_APPS;
                else
                    snap->depth_mode = SM_DEPTH_ALL_FILES;
            }
            continue;
        }
        if (strcmp(line, "[software]") == 0) {
            section = 1;
            continue;
        }
        if (strcmp(line, "[tree]") == 0) {
            section = 2;
            continue;
        }
        if (section == 1) {
            char *p = line;
            char *scope = tab_field(&p);
            char *name = tab_field(&p);
            char *ver = tab_field(&p);
            char *loc = tab_field(&p);
            char *pub = tab_field(&p);
            if (!name || !*name)
                continue;
            sm_software_t *sw = sm_software_new();
            if (!sw)
                continue;
            sw->scope = (scope && strcmp(scope, "HKCU") == 0) ? 1 : 0;
            sw->name = sm_strdup(name);
            sw->version = sm_strdup(ver ? ver : "");
            sw->location = sm_strdup(loc ? loc : "");
            sw->publisher = sm_strdup(pub ? pub : "");
            sm_snapshot_add_software(snap, sw);
        } else if (section == 2) {
            char *p = line;
            char *type = tab_field(&p);
            char *npath = tab_field(&p);
            if (!type || !npath || !*npath)
                continue;
            sm_node_type_t t = SM_FILE;
            if (type[0] == 'D')
                t = SM_DIR;
            else if (type[0] == 'L')
                t = SM_LINK;
            sm_snapshot_add_node(snap, t, npath, 0, 0);
        }
    }
    fclose(fp);
    if (!have_scan)
        snap->scan_time = sm_now_unix();
    return 0;
}

/* ---- binary format ---- */

int sm_snapshot_save_smb(const sm_snapshot_t *snap, const char *path) {
    FILE *fp = fopen(path, "wb");
    if (!fp) {
        sm_log(SM_LOG_ERROR, "cannot write %s", path);
        return -1;
    }
    uint8_t header[64];
    memset(header, 0, sizeof(header));
    memcpy(header, SM_MAGIC, 8);
    header[8] = 1;
    header[9] = 0; /* version */
    /* header[10]: depth_mode (0=all_files, 1=folders_and_apps) */
    header[10] = (uint8_t)snap->depth_mode;
    /* flags: bit0 size, bit1 mtime - we store size/mtime as 0 always for now */
    uint32_t flags = 0;
    header[12] = (uint8_t)(flags);
    header[13] = (uint8_t)(flags >> 8);
    header[14] = (uint8_t)(flags >> 16);
    header[15] = (uint8_t)(flags >> 24);

    uint64_t st = (uint64_t)snap->scan_time;
    int i;
    for (i = 0; i < 8; ++i)
        header[16 + i] = (uint8_t)((st >> (8 * i)) & 0xff);

    uint32_t swc = snap->software_count;
    for (i = 0; i < 4; ++i)
        header[24 + i] = (uint8_t)((swc >> (8 * i)) & 0xff);

    uint64_t nc = snap->node_count;
    for (i = 0; i < 8; ++i)
        header[32 + i] = (uint8_t)((nc >> (8 * i)) & 0xff);

    uint32_t dc = snap->dir_count;
    for (i = 0; i < 4; ++i)
        header[40 + i] = (uint8_t)((dc >> (8 * i)) & 0xff);
    uint32_t fc = snap->file_count;
    for (i = 0; i < 4; ++i)
        header[44 + i] = (uint8_t)((fc >> (8 * i)) & 0xff);

    if (snap->hostname) {
        size_t hn = strlen(snap->hostname);
        if (hn > 15)
            hn = 15;
        memcpy(header + 48, snap->hostname, hn);
    }
    if (fwrite(header, 1, 64, fp) != 64) {
        fclose(fp);
        return -1;
    }

    for (const sm_software_t *sw = snap->software; sw; sw = sw->next) {
        if (write_u8(fp, (uint8_t)sw->scope) != 0 ||
            write_str(fp, sw->name) != 0 || write_str(fp, sw->version) != 0 ||
            write_str(fp, sw->location) != 0 ||
            write_str(fp, sw->publisher) != 0) {
            fclose(fp);
            return -1;
        }
    }

    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (write_u8(fp, (uint8_t)n->type) != 0 || write_str(fp, n->path) != 0) {
            fclose(fp);
            return -1;
        }
    }
    fclose(fp);
    return 0;
}

int sm_snapshot_load_smb(sm_snapshot_t *snap, const char *path) {
    FILE *fp = fopen(path, "rb");
    if (!fp) {
        sm_log(SM_LOG_ERROR, "cannot read %s", path);
        return -1;
    }
    uint8_t header[64];
    if (fread(header, 1, 64, fp) != 64) {
        fclose(fp);
        return -1;
    }
    if (memcmp(header, SM_MAGIC, 7) != 0) {
        sm_log(SM_LOG_ERROR, "bad magic in %s", path);
        fclose(fp);
        return -1;
    }
    sm_snapshot_init(snap);
    uint64_t st = 0;
    int i;
    for (i = 0; i < 8; ++i)
        st |= ((uint64_t)header[16 + i]) << (8 * i);
    snap->scan_time = (int64_t)st;

    if (header[10] == (uint8_t)SM_DEPTH_FOLDERS_AND_APPS)
        snap->depth_mode = SM_DEPTH_FOLDERS_AND_APPS;
    else
        snap->depth_mode = SM_DEPTH_ALL_FILES;

    uint32_t swc = 0;
    for (i = 0; i < 4; ++i)
        swc |= ((uint32_t)header[24 + i]) << (8 * i);

    uint64_t nc = 0;
    for (i = 0; i < 8; ++i)
        nc |= ((uint64_t)header[32 + i]) << (8 * i);

    char host[16];
    memcpy(host, header + 48, 15);
    host[15] = '\0';
    if (host[0])
        snap->hostname = sm_strdup(host);

    uint32_t si;
    for (si = 0; si < swc; ++si) {
        uint8_t scope = 0;
        if (read_u8(fp, &scope) != 0) {
            fclose(fp);
            sm_snapshot_free(snap);
            return -1;
        }
        sm_software_t *sw = sm_software_new();
        if (!sw) {
            fclose(fp);
            sm_snapshot_free(snap);
            return -1;
        }
        sw->scope = scope;
        sw->name = read_str(fp);
        sw->version = read_str(fp);
        sw->location = read_str(fp);
        sw->publisher = read_str(fp);
        if (!sw->name) {
            sm_software_free(sw);
            fclose(fp);
            sm_snapshot_free(snap);
            return -1;
        }
        sm_snapshot_add_software(snap, sw);
    }

    uint64_t ni;
    for (ni = 0; ni < nc; ++ni) {
        uint8_t type = 0;
        if (read_u8(fp, &type) != 0) {
            fclose(fp);
            sm_snapshot_free(snap);
            return -1;
        }
        char *npath = read_str(fp);
        if (!npath) {
            fclose(fp);
            sm_snapshot_free(snap);
            return -1;
        }
        sm_snapshot_add_node(snap, (sm_node_type_t)type, npath, 0, 0);
        free(npath);
    }
    fclose(fp);
    return 0;
}

int sm_snapshot_save(const sm_snapshot_t *snap, const char *path) {
    if (ends_with_ci(path, ".smap"))
        return sm_snapshot_save_smap(snap, path);
    return sm_snapshot_save_smb(snap, path);
}

int sm_snapshot_load(sm_snapshot_t *snap, const char *path) {
    if (ends_with_ci(path, ".smap"))
        return sm_snapshot_load_smap(snap, path);
    return sm_snapshot_load_smb(snap, path);
}
