#ifndef SOFTMAP_FILTER_H
#define SOFTMAP_FILTER_H

#include "softmap/types.h"

/* Exclude if any path component matches an exclude name (case-insensitive). */
int sm_should_exclude(const sm_config_t *cfg, const char *path, const char *name);

#endif /* SOFTMAP_FILTER_H */
