#include "softmap/cmd.h"
#include "softmap/restore.h"
#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int cmd_restore(int argc, char **argv) {
    const char *snap_path = NULL;
    const char *target = NULL;
    int dry_run = 0;
    int yes = 0;
    sm_path_map_t maps[SM_MAX_MAPS];
    int map_count = 0;
    int i;

    memset(maps, 0, sizeof(maps));

    for (i = 0; i < argc; ++i) {
        if (strcmp(argv[i], "--target") == 0 && i + 1 < argc) {
            target = argv[++i];
        } else if (strcmp(argv[i], "--dry-run") == 0) {
            dry_run = 1;
        } else if (strcmp(argv[i], "-y") == 0 || strcmp(argv[i], "--yes") == 0) {
            yes = 1;
        } else if (strcmp(argv[i], "--dirs-only") == 0) {
            /* default behavior; accepted for clarity */
        } else if (strcmp(argv[i], "--map") == 0 && i + 1 < argc) {
            char *spec = argv[++i];
            char *eq = strchr(spec, '=');
            if (!eq || map_count >= SM_MAX_MAPS) {
                sm_log(SM_LOG_ERROR, "invalid --map (use old=new)");
                return 1;
            }
            *eq = '\0';
            maps[map_count].from = spec;
            maps[map_count].to = eq + 1;
            map_count++;
        } else if (strcmp(argv[i], "-h") == 0 || strcmp(argv[i], "--help") == 0) {
            printf("Usage: softmap restore <snapshot> --target DIR "
                   "[--dry-run] [--map old=new] [-y]\n");
            return 0;
        } else if (argv[i][0] == '-') {
            sm_log(SM_LOG_ERROR, "unknown restore option: %s", argv[i]);
            return 1;
        } else if (!snap_path) {
            snap_path = argv[i];
        } else {
            sm_log(SM_LOG_ERROR, "unexpected argument: %s", argv[i]);
            return 1;
        }
    }

    if (!snap_path || !target) {
        sm_log(SM_LOG_ERROR, "restore requires <snapshot> and --target");
        return 1;
    }

    sm_snapshot_t snap;
    if (sm_snapshot_load(&snap, snap_path) != 0)
        return 2;

    int rc = sm_restore_dirs(&snap, target, maps, map_count, dry_run, yes);
    sm_snapshot_free(&snap);
    return rc;
}
