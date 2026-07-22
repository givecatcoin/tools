#ifndef SOFTMAP_CONFIG_H
#define SOFTMAP_CONFIG_H

#include "softmap/types.h"

void sm_config_init_defaults(sm_config_t *cfg);
void sm_config_free(sm_config_t *cfg);
int sm_config_load(sm_config_t *cfg, const char *path); /* 0=ok, 1=missing */
int sm_config_add_drive(sm_config_t *cfg, const char *drive);
int sm_config_add_exclude(sm_config_t *cfg, const char *name);
/* Enumerate local drives into cfg (clears existing drive list first). */
int sm_config_detect_drives(sm_config_t *cfg);

#endif /* SOFTMAP_CONFIG_H */
