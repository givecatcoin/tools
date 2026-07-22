#ifndef SOFTMAP_UTIL_H
#define SOFTMAP_UTIL_H

#include "softmap/types.h"

#include <stddef.h>
#include <stdint.h>

void sm_set_log_level(sm_log_level_t level);
void sm_log(sm_log_level_t level, const char *fmt, ...);

char *sm_strdup(const char *s);
char *sm_strndup(const char *s, size_t n);
void sm_free(void *p);

int64_t sm_now_unix(void);
void sm_format_time(int64_t t, char *buf, size_t buflen);
int sm_parse_time(const char *s, int64_t *out);

const char *sm_basename(const char *path);
int sm_path_has_ext(const char *path, const char *ext);
void sm_path_normalize_sep(char *path);

int sm_strcasecmp_ascii(const char *a, const char *b);
int sm_str_starts_with_ci(const char *s, const char *prefix);

#endif /* SOFTMAP_UTIL_H */
