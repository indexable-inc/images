# Copyright 2025 Tomeu Vizoso <tomeu@tomeuvizoso.net>
# SPDX-License-Identifier: MIT

files_mlw_codec = files(
   'source/mlw_encode.cpp',
   'source/mlw_encode_fwd.cpp',
   'source/ml_ethosu_encode.cpp',
   'source/mlw_decode.cpp',
)

libmlw_codec = static_library(
  'mlw_codec',
  [files_mlw_codec],
  gnu_symbol_visibility : 'hidden',
)

inc_libmlw_codec = [include_directories('include')]