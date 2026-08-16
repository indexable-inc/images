/* Copyright © 2025 Intel Corporation
 * SPDX-License-Identifier: MIT
 */

#include "compiler/libcl/libcl.h"
#include "compiler/nir/nir_defines.h"
#include "compiler/shader_enums.h"

bool
blorp_check_in_bounds(uint4 bounds_rect, uint2 pos)
{
   uint x0 = bounds_rect[0], x1 = bounds_rect[1];
   uint y0 = bounds_rect[2], y1 = bounds_rect[3];

   return pos.x >= x0 && pos.x < x1 &&
          pos.y >= y0 && pos.y < y1;
}
