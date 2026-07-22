#ifndef SOFTMAP_WALKER_H
#define SOFTMAP_WALKER_H

#include "softmap/types.h"

/* BF2: recursively walk configured drives into snapshot. */
int sm_walk_drives(sm_snapshot_t *snap, const sm_config_t *cfg);

#endif /* SOFTMAP_WALKER_H */
