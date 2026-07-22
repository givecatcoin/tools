#ifndef SOFTMAP_REPORT_H
#define SOFTMAP_REPORT_H

#include "softmap/types.h"

#include <stdio.h>

void sm_report_summary(const sm_snapshot_t *snap, FILE *out);
void sm_report_software(const sm_snapshot_t *snap, FILE *out);
void sm_report_tools(const sm_snapshot_t *snap, FILE *out);
void sm_report_checklist(const sm_snapshot_t *snap, FILE *out);
void sm_report_tree(const sm_snapshot_t *snap, FILE *out, int max_depth);

#endif /* SOFTMAP_REPORT_H */
