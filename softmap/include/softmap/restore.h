#ifndef SOFTMAP_RESTORE_H
#define SOFTMAP_RESTORE_H

#include "softmap/types.h"

int sm_restore_dirs(const sm_snapshot_t *snap, const char *target,
                    const sm_path_map_t *maps, int map_count, int dry_run,
                    int yes);

#endif /* SOFTMAP_RESTORE_H */
