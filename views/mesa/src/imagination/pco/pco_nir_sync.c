/*
 * Copyright © 2025 Imagination Technologies Ltd.
 *
 * SPDX-License-Identifier: MIT
 */

/**
 * \file pco_nir_sync.c
 *
 * \brief PCO NIR sync-related passes.
 */

#include "hwdef/rogue_hw_defs.h"
#include "nir.h"
#include "nir_builder.h"
#include "nir_builtin_builder.h"
#include "pco.h"
#include "pco_builder.h"
#include "pco_internal.h"
#include "pco_usclib.h"
#include "util/macros.h"

#include <assert.h>
#include <stdbool.h>
#include <stdio.h>

/**
 * \brief Lowers a barrier instruction.
 *
 * \param[in] b NIR builder.
 * \param[in] instr NIR instruction.
 * \param[in] cb_data User callback data.
 * \return The replacement/lowered def.
 */
static nir_def *lower_barrier(nir_builder *b, nir_instr *instr, void *cb_data)
{
   struct shader_info *info = &b->shader->info;
   bool *uses_usclib = cb_data;

   nir_intrinsic_instr *intr = nir_instr_as_intrinsic(instr);
   mesa_scope exec_scope = nir_intrinsic_execution_scope(intr);

   unsigned wg_size = info->workgroup_size[0] * info->workgroup_size[1] *
                      info->workgroup_size[2];

   if (wg_size <= ROGUE_MAX_INSTANCES_PER_TASK || exec_scope == SCOPE_NONE ||
       exec_scope == SCOPE_SUBGROUP)
      return NIR_LOWER_INSTR_PROGRESS_REPLACE;

   /* TODO: We might be able to re-use barrier counters. */
   unsigned counter_offset = info->shared_size;
   info->shared_size += sizeof(uint32_t);
   info->zero_initialize_shared_memory = true;

   *uses_usclib = true;

   unsigned num_slots = DIV_ROUND_UP(wg_size, ROGUE_MAX_INSTANCES_PER_TASK);

   b->cursor = nir_before_instr(instr);
   usclib_barrier(b, nir_imm_int(b, num_slots), nir_imm_int(b, counter_offset));

   return NIR_LOWER_INSTR_PROGRESS_REPLACE;
}

/**
 * \brief Filters barrier instructions.
 *
 * \param[in] instr NIR instruction.
 * \param[in] cb_data User callback data.
 * \return True if the instruction matches the filter.
 */
static bool is_barrier(const nir_instr *instr, UNUSED const void *cb_data)
{
   if (instr->type != nir_instr_type_intrinsic)
      return false;

   return nir_instr_as_intrinsic(instr)->intrinsic == nir_intrinsic_barrier;
}

/**
 * \brief Barrier lowering pass.
 *
 * \param[in,out] shader NIR shader.
 * \return True if the pass made progress.
 */
bool pco_nir_lower_barriers(nir_shader *shader, pco_data *data)
{
   bool progress = nir_shader_lower_instructions(shader,
                                                 is_barrier,
                                                 lower_barrier,
                                                 &data->common.uses.usclib);

   data->common.uses.barriers |= progress;

   return progress;
}

static nir_def *
lower_usclib_atomic(nir_builder *b, nir_instr *instr, void *cb_data)
{
   nir_intrinsic_instr *intr = nir_instr_as_intrinsic(instr);
   bool *uses_usclib = cb_data;

   b->cursor = nir_before_instr(instr);

   if (intr->intrinsic == nir_intrinsic_ssbo_atomic_swap) {
      nir_def *buffer = intr->src[0].ssa;
      nir_def *offset = intr->src[1].ssa;
      nir_def *value = intr->src[2].ssa;
      nir_def *value_swap = intr->src[3].ssa;

      ASSERTED enum gl_access_qualifier access = nir_intrinsic_access(intr);
      ASSERTED unsigned num_components = intr->def.num_components;
      ASSERTED unsigned bit_size = intr->def.bit_size;
      assert(access == ACCESS_COHERENT);
      assert(num_components == 1 && bit_size == 32);

      *uses_usclib = true;
      return usclib_emu_ssbo_atomic_comp_swap(b,
                                              buffer,
                                              offset,
                                              value,
                                              value_swap);
   }

   nir_def *addr_data = intr->src[0].ssa;
   nir_def *addr = nir_channels(b, addr_data, BITFIELD_RANGE(0, 2));
   nir_def *value = nir_channel(b, addr_data, 2);
   nir_def *value_swap = nir_channel(b, addr_data, 3);

   ASSERTED unsigned num_components = intr->def.num_components;
   ASSERTED unsigned bit_size = intr->def.bit_size;
   assert(num_components == 1 && bit_size == 32);

   *uses_usclib = true;
   return usclib_emu_global_atomic_comp_swap(b, addr, value, value_swap);
}

static bool lower_global_atomic_intrinsic(nir_builder *b,
                                          nir_intrinsic_instr *intrin,
                                          UNUSED void *data)
{
   nir_intrinsic_op op;
   bool is_swap = false;
   switch (intrin->intrinsic) {
   case nir_intrinsic_global_atomic_2x32:
      op = nir_intrinsic_global_atomic_pco;
      break;
   case nir_intrinsic_global_atomic_swap_2x32:
      op = nir_intrinsic_global_atomic_swap_pco;
      is_swap = true;
      break;
   default:
      return false;
   }

   nir_src *addr_src = &intrin->src[0];
   nir_src *data_src = &intrin->src[1];
   nir_src *data2_src;
   if (is_swap)
      data2_src = &intrin->src[2];

   b->cursor = nir_before_instr(&intrin->instr);

   nir_def *new_src = nir_pad_vector(b, addr_src->ssa, is_swap ? 4 : 3);
   new_src = nir_vector_insert_imm(b, new_src, data_src->ssa, 2);
   if (is_swap)
      new_src = nir_vector_insert_imm(b, new_src, data2_src->ssa, 3);

   nir_intrinsic_instr *new_intrin = nir_intrinsic_instr_create(b->shader, op);
   new_intrin->num_components = intrin->num_components;
   nir_def_init(&new_intrin->instr,
                &new_intrin->def,
                intrin->def.num_components,
                intrin->def.bit_size);
   new_intrin->src[0] = nir_src_for_ssa(new_src);
   assert(nir_intrinsic_has_atomic_op(intrin));
   nir_intrinsic_set_atomic_op(new_intrin, nir_intrinsic_atomic_op(intrin));

   nir_builder_instr_insert(b, &new_intrin->instr);
   nir_def_rewrite_uses(&intrin->def, &new_intrin->def);
   nir_instr_remove(&intrin->instr);

   return true;
}

/**
 * \brief Filters atomic instructions emulated with usclib.
 *
 * \param[in] instr NIR instruction.
 * \param[in] cb_data User callback data.
 * \return True if the instruction matches the filter.
 */
static bool atomic_uses_usclib(const nir_instr *instr,
                               UNUSED const void *cb_data)
{
   if (instr->type != nir_instr_type_intrinsic)
      return false;

   nir_intrinsic_instr *intr = nir_instr_as_intrinsic(instr);

   return intr->intrinsic == nir_intrinsic_ssbo_atomic_swap ||
          intr->intrinsic == nir_intrinsic_global_atomic_swap_pco;
}

/**
 * \brief Atomics lowering pass.
 *
 * \param[in,out] shader NIR shader.
 * \return True if the pass made progress.
 */
bool pco_nir_lower_atomics(nir_shader *shader, pco_data *data)
{
   bool progress = false;

   progress |= nir_shader_intrinsics_pass(shader,
                                          lower_global_atomic_intrinsic,
                                          nir_metadata_none,
                                          NULL);
   progress |= nir_shader_lower_instructions(shader,
                                             atomic_uses_usclib,
                                             lower_usclib_atomic,
                                             &data->common.uses.usclib);

   return progress;
}

static nir_def *
lower_subgroup_intrinsic(nir_builder *b, nir_instr *instr, void *cb_data)
{
   nir_intrinsic_instr *intr = nir_instr_as_intrinsic(instr);
   assert(intr->def.num_components == 1);

   switch (intr->intrinsic) {
   case nir_intrinsic_load_subgroup_size:
      return nir_imm_int(b, 1);

   case nir_intrinsic_load_subgroup_invocation:
      return nir_imm_int(b, 0);

   case nir_intrinsic_load_num_subgroups:
      return nir_imm_int(b,
                         b->shader->info.workgroup_size[0] *
                            b->shader->info.workgroup_size[1] *
                            b->shader->info.workgroup_size[2]);

   case nir_intrinsic_load_subgroup_id:
      return nir_load_local_invocation_index(b);

   case nir_intrinsic_first_invocation:
      return nir_imm_int(b, 0);

   case nir_intrinsic_elect:
      return nir_imm_true(b);

   default:
      break;
   }

   UNREACHABLE("");
}

static bool is_subgroup_intrinsic(const nir_instr *instr,
                                  UNUSED const void *cb_data)
{
   if (instr->type != nir_instr_type_intrinsic)
      return false;

   nir_intrinsic_instr *intr = nir_instr_as_intrinsic(instr);
   switch (intr->intrinsic) {
   case nir_intrinsic_load_subgroup_size:
   case nir_intrinsic_load_subgroup_invocation:
   case nir_intrinsic_load_num_subgroups:
   case nir_intrinsic_load_subgroup_id:
   case nir_intrinsic_first_invocation:
   case nir_intrinsic_elect:
      return true;

   default:
      break;
   }

   return false;
}

bool pco_nir_lower_subgroups(nir_shader *shader)
{
   shader->info.api_subgroup_size = 1;
   shader->info.min_subgroup_size = 1;
   shader->info.max_subgroup_size = 1;

   return nir_shader_lower_instructions(shader,
                                        is_subgroup_intrinsic,
                                        lower_subgroup_intrinsic,
                                        NULL);
}
