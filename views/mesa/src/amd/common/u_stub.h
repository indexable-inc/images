/*
 * Copyright 2026 Advanced Micro Devices, Inc.
 *
 * SPDX-License-Identifier: MIT
 * 
 * Helpers macro to stub out some functions by replacing the declaration
 * with an empty stub.
 * The typical usage is:
 *
 *    #ifndef HAVE_XXXX
 *    #define __U_STUB__
 *    #endif
 *    #include "u_stub.h"
 *
 */

#undef PROC
#undef TAIL
#undef TAILV
#undef TAILB
#undef TAILBT
#undef TAILPTR

#ifdef __U_STUB__
#define PROC static inline
#define TAIL                                                                                       \
   {                                                                                               \
      return -1;                                                                                   \
   }
#define TAILV                                                                                      \
   {                                                                                               \
   }
#define TAILPTR                                                                                    \
   {                                                                                               \
      return NULL;                                                                                 \
   }
#define TAILB                                                                                      \
   {                                                                                               \
      return false;                                                                                \
   }
#define TAILBT                                                                                     \
   {                                                                                               \
      return true;                                                                                 \
   }
#else
#define PROC
#define TAIL
#define TAILV
#define TAILB
#define TAILBT
#define TAILPTR
#endif

#undef __U_STUB__