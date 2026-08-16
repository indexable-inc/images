/*
 * Copyright 2026 Advanced Micro Devices, Inc.
 *
 * SPDX-License-Identifier: MIT
 */

#ifndef AC_VIDEO_H
#define AC_VIDEO_H

#include "ac_surface.h"

enum ac_video_subsample {
   AC_VIDEO_SUBSAMPLE_420 = 0x0,
   AC_VIDEO_SUBSAMPLE_422 = 0x1,
   AC_VIDEO_SUBSAMPLE_444 = 0x2,
   AC_VIDEO_SUBSAMPLE_400 = 0x3,
};

enum ac_video_codec {
   AC_VIDEO_CODEC_AVC,
   AC_VIDEO_CODEC_HEVC,
   AC_VIDEO_CODEC_VP9,
   AC_VIDEO_CODEC_AV1,
   AC_VIDEO_CODEC_MJPEG,
   AC_VIDEO_CODEC_MPEG2,
   AC_VIDEO_CODEC_VC1,
};

struct ac_video_surface {
   enum pipe_format format;
   uint64_t size;
   uint32_t num_planes;
   struct {
      uint64_t va;
      struct radeon_surf *surf;
   } planes[3];
};

#endif
