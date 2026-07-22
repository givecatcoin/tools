#ifndef SOFTMAP_TYPES_H
#define SOFTMAP_TYPES_H

#include <stddef.h>
#include <stdint.h>

#define SM_VERSION_MAJOR 1
#define SM_VERSION_MINOR 0
#define SM_MAGIC "SMAP001"

#define SM_MAX_EXCLUDES 64
#define SM_MAX_DRIVES 26
#define SM_MAX_PATH 4096
#define SM_MAX_MAPS 32

typedef enum {
    SM_DIR = 0,
    SM_FILE = 1,
    SM_LINK = 2
} sm_node_type_t;

typedef enum {
    SM_DEPTH_ALL_FILES = 0,
    SM_DEPTH_FOLDERS_AND_APPS = 1
} sm_depth_mode_t;

typedef enum {
    SM_LOG_ERROR = 0,
    SM_LOG_INFO = 1,
    SM_LOG_VERBOSE = 2
} sm_log_level_t;

typedef struct sm_software {
    char *name;
    char *version;
    char *publisher;
    char *location;
    char *uninstall_key;
    int scope; /* 0=HKLM, 1=HKCU */
    struct sm_software *next;
} sm_software_t;

typedef struct sm_node {
    sm_node_type_t type;
    char *path;
    char *name;
    uint64_t size;
    int64_t mtime;
    struct sm_node *next;
} sm_node_t;

typedef struct sm_snapshot {
    char *hostname;
    int64_t scan_time;
    sm_software_t *software;
    uint32_t software_count;
    sm_node_t *nodes;
    sm_node_t *nodes_tail;
    uint64_t node_count;
    uint32_t dir_count;
    uint32_t file_count;
    sm_depth_mode_t depth_mode;
} sm_snapshot_t;

typedef struct sm_config {
    char *drives[SM_MAX_DRIVES];
    int drive_count;
    char *excludes[SM_MAX_EXCLUDES];
    int exclude_count;
    sm_depth_mode_t depth;
    int software_only;
} sm_config_t;

typedef struct sm_path_map {
    char *from;
    char *to;
} sm_path_map_t;

#endif /* SOFTMAP_TYPES_H */
