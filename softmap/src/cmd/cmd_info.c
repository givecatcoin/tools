#include "softmap/cmd.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <string.h>

int cmd_info(int argc, char **argv) {
    if (argc < 1) {
        sm_log(SM_LOG_ERROR, "info requires snapshot path");
        return 1;
    }
    sm_snapshot_t snap;
    if (sm_snapshot_load(&snap, argv[0]) != 0)
        return 2;
    char tbuf[64];
    sm_format_time(snap.scan_time, tbuf, sizeof(tbuf));
    printf("host: %s\n", snap.hostname ? snap.hostname : "");
    printf("scan: %s\n", tbuf);
    printf("depth: %s\n",
           snap.depth_mode == SM_DEPTH_FOLDERS_AND_APPS ? "folders_and_apps"
                                                       : "all_files");
    printf("software: %u\n", snap.software_count);
    printf("nodes: %llu\n", (unsigned long long)snap.node_count);
    printf("dirs: %u\n", snap.dir_count);
    printf("files: %u\n", snap.file_count);
    sm_snapshot_free(&snap);
    return 0;
}
