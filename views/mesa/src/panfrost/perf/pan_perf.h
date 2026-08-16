/*
 * Copyright © 2021 Collabora, Ltd.
 * SPDX-License-Identifier: MIT
 */

#ifndef PAN_PERF_H
#define PAN_PERF_H

#include <stdint.h>

#if defined(__cplusplus)
extern "C" {
#endif

#define PAN_PERF_MAX_CATEGORIES 4
#define PAN_PERF_MAX_COUNTERS   64

struct pan_kmod_dev;
struct pan_kmod_dev_props;
struct pan_model;
struct pan_perf_category;
struct pan_perf;

enum pan_perf_counter_units {
   PAN_PERF_COUNTER_UNITS_CYCLES,
   PAN_PERF_COUNTER_UNITS_JOBS,
   PAN_PERF_COUNTER_UNITS_TASKS,
   PAN_PERF_COUNTER_UNITS_PRIMITIVES,
   PAN_PERF_COUNTER_UNITS_BEATS,
   PAN_PERF_COUNTER_UNITS_REQUESTS,
   PAN_PERF_COUNTER_UNITS_WARPS,
   PAN_PERF_COUNTER_UNITS_QUADS,
   PAN_PERF_COUNTER_UNITS_TILES,
   PAN_PERF_COUNTER_UNITS_INSTRUCTIONS,
   PAN_PERF_COUNTER_UNITS_TRANSACTIONS,
   PAN_PERF_COUNTER_UNITS_THREADS,
   PAN_PERF_COUNTER_UNITS_BYTES,
   PAN_PERF_COUNTER_UNITS_PIXELS,
   PAN_PERF_COUNTER_UNITS_ISSUES,
};

struct pan_perf_counter {
   const char *name;
   const char *desc;
   const char *symbol_name;
   enum pan_perf_counter_units units;
   // Offset of this counter's value within the category
   uint32_t offset;
   unsigned category_index;
};

struct pan_perf_category {
   const char *name;

   struct pan_perf_counter counters[PAN_PERF_MAX_COUNTERS];
   uint32_t n_counters;

   /* Offset of this category within the counters memory block */
   unsigned offset;
};

struct pan_perf_config {
   const char *name;

   struct pan_perf_category categories[PAN_PERF_MAX_CATEGORIES];
   uint32_t n_categories;
};

struct pan_perf {
   struct pan_kmod_dev *dev;
   unsigned core_id_range;
   const struct pan_perf_config *cfg;

   // Memory where to dump counter values
   uint32_t *counter_values;
   uint32_t n_counter_values;

   /* Offsets of categories */
   unsigned category_offset[PAN_PERF_MAX_CATEGORIES];
};

uint32_t pan_perf_counter_read(const struct pan_perf_counter *counter,
                               const struct pan_perf *perf);

void pan_perf_init(struct pan_perf *perf, int fd);

int pan_perf_enable(struct pan_perf *perf);

int pan_perf_disable(struct pan_perf *perf);

int pan_perf_dump(struct pan_perf *perf);

#if defined(__cplusplus)
} // extern "C"
#endif

#endif // PAN_PERF_H
