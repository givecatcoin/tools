#ifndef SOFTMAP_REGISTRY_H
#define SOFTMAP_REGISTRY_H

#include "softmap/types.h"

/* BF1: scan Windows Uninstall registry keys into snapshot. */
int sm_registry_scan(sm_snapshot_t *snap);

#endif /* SOFTMAP_REGISTRY_H */
