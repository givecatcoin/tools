#include "softmap/snapshot.h"
#include "softmap/util.h"

#include <stdlib.h>
#include <string.h>

void sm_snapshot_init(sm_snapshot_t *snap) {
    memset(snap, 0, sizeof(*snap));
    snap->depth_mode = SM_DEPTH_ALL_FILES;
}

sm_software_t *sm_software_new(void) {
    sm_software_t *sw = (sm_software_t *)calloc(1, sizeof(*sw));
    return sw;
}

void sm_software_free(sm_software_t *sw) {
    while (sw) {
        sm_software_t *n = sw->next;
        sm_free(sw->name);
        sm_free(sw->version);
        sm_free(sw->publisher);
        sm_free(sw->location);
        sm_free(sw->uninstall_key);
        free(sw);
        sw = n;
    }
}

static void free_nodes(sm_node_t *n) {
    while (n) {
        sm_node_t *next = n->next;
        sm_free(n->path);
        sm_free(n->name);
        free(n);
        n = next;
    }
}

void sm_snapshot_free(sm_snapshot_t *snap) {
    if (!snap)
        return;
    sm_free(snap->hostname);
    sm_software_free(snap->software);
    free_nodes(snap->nodes);
    memset(snap, 0, sizeof(*snap));
}

int sm_snapshot_add_software(sm_snapshot_t *snap, sm_software_t *sw) {
    if (!snap || !sw)
        return -1;
    sw->next = NULL;
    if (!snap->software) {
        snap->software = sw;
    } else {
        sm_software_t *p = snap->software;
        while (p->next)
            p = p->next;
        p->next = sw;
    }
    snap->software_count++;
    return 0;
}

int sm_snapshot_add_node(sm_snapshot_t *snap, sm_node_type_t type,
                         const char *path, uint64_t size, int64_t mtime) {
    if (!snap || !path)
        return -1;
    sm_node_t *n = (sm_node_t *)calloc(1, sizeof(*n));
    if (!n)
        return -1;
    n->type = type;
    n->path = sm_strdup(path);
    n->name = sm_strdup(sm_basename(path));
    n->size = size;
    n->mtime = mtime;
    n->next = NULL;
    if (!n->path || !n->name) {
        sm_free(n->path);
        sm_free(n->name);
        free(n);
        return -1;
    }

    if (!snap->nodes) {
        snap->nodes = n;
        snap->nodes_tail = n;
    } else {
        snap->nodes_tail->next = n;
        snap->nodes_tail = n;
    }
    snap->node_count++;
    if (type == SM_DIR)
        snap->dir_count++;
    else
        snap->file_count++;
    return 0;
}
