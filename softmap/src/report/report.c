#include "softmap/report.h"
#include "softmap/util.h"

#include <stdio.h>
#include <string.h>

static int is_tool_node(const sm_node_t *n) {
    if (!n || !n->path)
        return 0;
    if (n->type == SM_LINK)
        return 1;
    if (n->type == SM_FILE &&
        (sm_path_has_ext(n->path, ".exe") || sm_path_has_ext(n->path, ".lnk")))
        return 1;
    return 0;
}

static uint32_t count_tools(const sm_snapshot_t *snap) {
    uint32_t c = 0;
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (is_tool_node(n))
            ++c;
    }
    return c;
}

static uint32_t count_scope(const sm_snapshot_t *snap, int scope) {
    uint32_t c = 0;
    for (const sm_software_t *sw = snap->software; sw; sw = sw->next) {
        if (sw->scope == scope)
            ++c;
    }
    return c;
}

void sm_report_summary(const sm_snapshot_t *snap, FILE *out) {
    char tbuf[64];
    sm_format_time(snap->scan_time, tbuf, sizeof(tbuf));
    uint32_t tools = count_tools(snap);

    fprintf(out, "=======================================\n");
    fprintf(out, " SoftMap report\n");
    fprintf(out, " scan: %s  host: %s\n", tbuf,
            snap->hostname ? snap->hostname : "(unknown)");
    fprintf(out, "=======================================\n\n");

    fprintf(out, "[software] %u (BF1: registry)\n", snap->software_count);
    fprintf(out, "  HKLM: %u / HKCU: %u\n\n", count_scope(snap, 0),
            count_scope(snap, 1));

    fprintf(out, "[tools] %u (BF2: exe/lnk extracted)\n", tools);
    int shown = 0;
    for (const sm_node_t *n = snap->nodes; n && shown < 8; n = n->next) {
        if (!is_tool_node(n))
            continue;
        fprintf(out, "  %s\n", n->path);
        ++shown;
    }
    if (tools > 8)
        fprintf(out, "  ... (%u more; use --tools)\n", tools - 8);
    fprintf(out, "\n");

    fprintf(out, "[stats]\n");
    fprintf(out, "  directories: %u / files: %u / nodes: %llu\n",
            snap->dir_count, snap->file_count,
            (unsigned long long)snap->node_count);
    fprintf(out, "  depth mode: %s\n",
            snap->depth_mode == SM_DEPTH_FOLDERS_AND_APPS ? "folders_and_apps"
                                                         : "all_files");
    fprintf(out, "\n");
    fprintf(out, "Tips: --software / --tools / --checklist / --tree\n");
}

void sm_report_software(const sm_snapshot_t *snap, FILE *out) {
    fprintf(out, "=== Software (BF1) ===\n");
    for (const sm_software_t *sw = snap->software; sw; sw = sw->next) {
        fprintf(out, "[%s] %s", sw->scope == 1 ? "HKCU" : "HKLM",
                sw->name ? sw->name : "");
        if (sw->version && *sw->version)
            fprintf(out, "  (%s)", sw->version);
        fprintf(out, "\n");
        if (sw->location && *sw->location)
            fprintf(out, "    %s\n", sw->location);
    }
}

void sm_report_tools(const sm_snapshot_t *snap, FILE *out) {
    fprintf(out, "=== Tools (BF2 exe/lnk) ===\n");
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (is_tool_node(n))
            fprintf(out, "%s\n", n->path);
    }
}

void sm_report_checklist(const sm_snapshot_t *snap, FILE *out) {
    fprintf(out, "=== Re-setup checklist ===\n\n");
    fprintf(out, "## Software reinstall (BF1)\n");
    for (const sm_software_t *sw = snap->software; sw; sw = sw->next) {
        fprintf(out, "  [ ] %s", sw->name ? sw->name : "");
        if (sw->version && *sw->version)
            fprintf(out, "  (%s)", sw->version);
        fprintf(out, "\n");
    }
    fprintf(out, "\n## Tools / portable (BF2 - copy manually)\n");
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        if (is_tool_node(n))
            fprintf(out, "  [ ] %s\n", n->path);
    }
    fprintf(out, "\n## Directories (top-level samples)\n");
    int shown = 0;
    for (const sm_node_t *n = snap->nodes; n && shown < 40; n = n->next) {
        if (n->type != SM_DIR)
            continue;
        /* shallow-ish: few separators */
        int seps = 0;
        for (const char *p = n->path; *p; ++p) {
            if (*p == '\\' || *p == '/')
                ++seps;
        }
        if (seps <= 2) {
            fprintf(out, "  [ ] %s\n", n->path);
            ++shown;
        }
    }
}

static int path_depth(const char *path) {
    int d = 0;
    for (const char *p = path; *p; ++p) {
        if (*p == '\\' || *p == '/')
            ++d;
    }
    return d;
}

void sm_report_tree(const sm_snapshot_t *snap, FILE *out, int max_depth) {
    if (max_depth <= 0)
        max_depth = 3;
    fprintf(out, "=== Tree (depth <= %d) ===\n", max_depth);
    for (const sm_node_t *n = snap->nodes; n; n = n->next) {
        int d = path_depth(n->path);
        if (d > max_depth)
            continue;
        for (int i = 0; i < d; ++i)
            fputs("  ", out);
        if (n->type == SM_DIR)
            fprintf(out, "%s\\\n", n->name ? n->name : "");
        else
            fprintf(out, "%s\n", n->name ? n->name : "");
    }
}
