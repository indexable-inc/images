/*
 * Copyright © 2014 Intel Corporation
 * SPDX-License-Identifier: MIT
 */

#pragma once

#include "compiler/brw_list.h"

#ifdef __cplusplus
extern "C" {
#endif

struct cfg_t;
struct brw_inst;
struct intel_device_info;

struct inst_group {
   struct brw_exec_node link;

   int offset;

   size_t error_length;
   char *error;

   /* Pointers to the basic block in the CFG if the instruction group starts
    * or ends a basic block.
    */
   struct bblock_t *block_start;
   struct bblock_t *block_end;

   /* Annotation for the generated IR. */
   const char *annotation;
};

struct disasm_info {
   struct brw_exec_list group_list;

   const struct brw_isa_info *isa;
   const struct cfg_t *cfg;

   /** Block index in the cfg. */
   int cur_block;
   bool use_tail;
};

void
dump_assembly(void *assembly, int start_offset, int end_offset,
              struct disasm_info *disasm, const unsigned *block_latency, FILE *f);

struct disasm_info *
disasm_initialize(const struct brw_isa_info *isa,
                  const struct cfg_t *cfg);

struct inst_group *
disasm_new_inst_group(struct disasm_info *disasm, int offset);

void
disasm_annotate(struct disasm_info *disasm,
                struct brw_inst *inst, int offset);

void
disasm_insert_error(struct disasm_info *disasm, int offset,
                    int inst_size, const char *error);

#ifdef __cplusplus
} /* extern "C" */
#endif
