#ifndef SOFTMAP_SNAPSHOT_H
#define SOFTMAP_SNAPSHOT_H

#include "softmap/types.h"

void sm_snapshot_init(sm_snapshot_t *snap);
void sm_snapshot_free(sm_snapshot_t *snap);

sm_software_t *sm_software_new(void);
void sm_software_free(sm_software_t *sw);

int sm_snapshot_add_software(sm_snapshot_t *snap, sm_software_t *sw);
int sm_snapshot_add_node(sm_snapshot_t *snap, sm_node_type_t type,
                         const char *path, uint64_t size, int64_t mtime);

int sm_snapshot_save(const sm_snapshot_t *snap, const char *path);
int sm_snapshot_load(sm_snapshot_t *snap, const char *path);
int sm_snapshot_save_smap(const sm_snapshot_t *snap, const char *path);
int sm_snapshot_load_smap(sm_snapshot_t *snap, const char *path);
int sm_snapshot_save_smb(const sm_snapshot_t *snap, const char *path);
int sm_snapshot_load_smb(sm_snapshot_t *snap, const char *path);

#endif /* SOFTMAP_SNAPSHOT_H */
