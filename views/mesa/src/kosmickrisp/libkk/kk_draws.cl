/*
 * Copyright 2026 LunarG, Inc.
 * Copyright 2026 Google LLC
 * Copyright 2024 Valve Corporation
 * SPDX-License-Identifier: MIT
 */
#include "compiler/libcl/libcl_vk.h"

/*
 * To implement drawIndirectCount generically, we dispatch a kernel to
 * clone-and-patch the indirect buffer, predicating out draws as appropriate.
 */
KERNEL(32)
libkk_predicate_indirect(global uint32_t *out, constant uint32_t *in,
                         constant uint32_t *draw_count, uint32_t stride_el,
                         uint indexed__2)
{
   uint draw = cl_global_id.x;
   uint words = indexed__2 ? 5 : 4;
   bool enabled = draw < *draw_count;
   out += draw * words;
   in += draw * stride_el;

   /* Copy enabled draws, zero predicated draws. */
   for (uint i = 0; i < words; ++i) {
      out[i] = enabled ? in[i] : 0;
   }
}
