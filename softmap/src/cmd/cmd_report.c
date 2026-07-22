#include "softmap/cmd.h"
#include "softmap/report.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int cmd_report(int argc, char **argv) {
    const char *snap_path = NULL;
    const char *out_path = NULL;
    int want_software = 0;
    int want_tools = 0;
    int want_checklist = 0;
    int want_tree = 0;
    int tree_depth = 3;
    int i;

    for (i = 0; i < argc; ++i) {
        if (strcmp(argv[i], "--software") == 0) {
            want_software = 1;
        } else if (strcmp(argv[i], "--tools") == 0) {
            want_tools = 1;
        } else if (strcmp(argv[i], "--checklist") == 0) {
            want_checklist = 1;
        } else if (strcmp(argv[i], "--tree") == 0) {
            want_tree = 1;
        } else if (strcmp(argv[i], "--depth") == 0 && i + 1 < argc) {
            tree_depth = atoi(argv[++i]);
        } else if ((strcmp(argv[i], "-O") == 0 ||
                    strcmp(argv[i], "--output") == 0) &&
                   i + 1 < argc) {
            out_path = argv[++i];
        } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
            printf("Usage: softmap report <snapshot> [--software] [--tools] "
                   "[--checklist] [--tree] [--depth N] [-O file]\n");
            return 0;
        } else if (argv[i][0] == '-') {
            sm_log(SM_LOG_ERROR, "unknown report option: %s", argv[i]);
            return 1;
        } else if (!snap_path) {
            snap_path = argv[i];
        } else {
            sm_log(SM_LOG_ERROR, "unexpected argument: %s", argv[i]);
            return 1;
        }
    }

    if (!snap_path) {
        sm_log(SM_LOG_ERROR, "report requires snapshot path");
        return 1;
    }

    sm_snapshot_t snap;
    if (sm_snapshot_load(&snap, snap_path) != 0)
        return 2;

    FILE *out = stdout;
    if (out_path) {
        out = fopen(out_path, "wb");
        if (!out) {
            sm_log(SM_LOG_ERROR, "cannot write %s", out_path);
            sm_snapshot_free(&snap);
            return 1;
        }
    }

    int any = want_software || want_tools || want_checklist || want_tree;
    if (!any)
        sm_report_summary(&snap, out);
    if (want_software)
        sm_report_software(&snap, out);
    if (want_tools)
        sm_report_tools(&snap, out);
    if (want_checklist)
        sm_report_checklist(&snap, out);
    if (want_tree)
        sm_report_tree(&snap, out, tree_depth);

    if (out != stdout)
        fclose(out);
    sm_snapshot_free(&snap);
    return 0;
}
