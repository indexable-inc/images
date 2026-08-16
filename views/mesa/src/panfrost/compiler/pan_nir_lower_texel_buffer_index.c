/*
 * Copyright (C) 2025 Arm, Ltd.
 *
 * Derived from pan_nir_lower_image_index.c which is:
 * Copyright (C) 2024 Collabora, Ltd.
 *
 * SPDX-License-Identifier: MIT
 */

#include "compiler/nir/nir_builder.h"
#include "pan_nir.h"

static bool
lower_texel_buffer_fetch_intr(struct nir_builder *b, nir_tex_instr *tex,
                              void *data)
{
   if (tex->op != nir_texop_txf || tex->sampler_dim != GLSL_SAMPLER_DIM_BUF)
      return false;

   b->cursor = nir_before_instr(&tex->instr);

   nir_def *index = NULL;
   unsigned index_src;
   for (index_src = 0; index_src < tex->num_srcs; ++index_src) {
      switch (tex->src[index_src].src_type) {
      case nir_tex_src_texture_offset:
         /* This should always be 0 as lower_index_to_offset is expected to be
          * set */
         assert(tex->texture_index == 0);
         index = tex->src[index_src].src.ssa;
         break;
      default:
         continue;
      }
   }

   unsigned attr_offset = *(unsigned *)data;
   if (!index)
      tex->texture_index += attr_offset;
   else {
      b->cursor = nir_before_instr(&tex->instr);
      index = nir_iadd_imm(b, index, attr_offset);
      nir_src_rewrite(&tex->src[index_src].src, index);
   }
   return true;
}

bool
pan_nir_lower_texel_buffer_fetch_index(nir_shader *shader,
                                       unsigned attrib_offset)
{
   return nir_shader_tex_pass(shader, lower_texel_buffer_fetch_intr,
                              nir_metadata_control_flow, &attrib_offset);
}
