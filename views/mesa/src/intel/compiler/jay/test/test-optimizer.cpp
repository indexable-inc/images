/*
 * Copyright 2026 Intel Corporation
 * SPDX-License-Identifier: MIT
 */

#include "util/lut.h"
#include "jay_builder.h"
#include "jay_ir.h"
#include "jay_private.h"
#include "jay_test.h"

#include <gtest/gtest.h>

static void
jay_optimize_and_dce(jay_shader *shader)
{
   JAY_PASS(shader, jay_opt_propagate_forwards);
   JAY_PASS(shader, jay_opt_propagate_backwards);
   JAY_PASS(shader, jay_opt_dead_code);
}

#define CASE(instr, expected)                                                  \
   INSTRUCTION_CASE(                                                           \
      {                                                                        \
         instr;                                                                \
         jay_UNIT_TEST_u32(b, out);                                            \
      },                                                                       \
      {                                                                        \
         expected;                                                             \
         jay_UNIT_TEST_u32(b, out);                                            \
      },                                                                       \
      jay_optimize_and_dce)

#define NEGCASE(instr) CASE(instr, instr)
#define UNIT           jay_UNIT_TEST_u32

#define NEG(x) jay_negate(x)

#define MOV(T, src0)                                                           \
   ({                                                                          \
      jay_def dst = jay_alloc_def(b, GPR, 1);                                  \
      jay_MODIFIER(b, T, dst, src0);                                           \
      dst;                                                                     \
   })

class Optimizer : public testing::Test {
 protected:
   Optimizer()
   {
      mem_ctx = ralloc_context(NULL);

      out = jay_scalar(GPR, 8);
      wx = jay_scalar(TEST_FILE, 1);
      wy = jay_scalar(TEST_FILE, 1);
      wz = jay_scalar(TEST_FILE, 1);
   }

   ~Optimizer()
   {
      ralloc_free(mem_ctx);
   }

   void *mem_ctx;

   jay_def out, wx, wy, wz;
};

static enum jay_type float_types[] = {
   JAY_TYPE_F16,
   JAY_TYPE_F32,
};

TEST_F(Optimizer, Copyprop)
{
   CASE(jay_ADD(b, JAY_TYPE_U32, out, wx, jay_MOV_u32(b, wy)),
        jay_ADD(b, JAY_TYPE_U32, out, wx, wy));

   CASE(jay_ADD(b, JAY_TYPE_U32, out, wx, jay_MOV_u32(b, wy)),
        jay_ADD(b, JAY_TYPE_U32, out, wx, wy));
}

TEST_F(Optimizer, FusedNeg)
{
   for (unsigned i = 0; i < ARRAY_SIZE(float_types); ++i) {
      enum jay_type T = float_types[i];

      CASE(jay_ADD(b, T, out, wx, MOV(T, NEG(wy))),
           jay_ADD(b, T, out, wx, NEG(wy)));

      CASE(jay_MUL(b, T, out, MOV(T, NEG(wy)), NEG(wx)),
           jay_MUL(b, T, out, NEG(wy), NEG(wx)));

      CASE(jay_MAD(b, T, out, MOV(T, NEG(wy)), wz, NEG(MOV(T, NEG(wx)))),
           jay_MAD(b, T, out, NEG(wy), wz, wx));
   }
}

TEST_F(Optimizer, SELToFloat)
{
   CASE(
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_LT, flag, 3, x);
         jay_SEL(b, JAY_TYPE_U32, out, wx, MOV(JAY_TYPE_F32, NEG(wy)), flag);
      },
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_LT, flag, 3, x);
         jay_SEL(b, JAY_TYPE_F32, out, wx, NEG(wy), flag);
      });
}

TEST_F(Optimizer, FusedNot)
{
   CASE(jay_BFN(b, out, wx, jay_NOT_u32(b, wy), 0, UTIL_LUT3(a & b)),
        jay_BFN(b, out, wx, wy, 0, UTIL_LUT3(a & ~b)));

   CASE(jay_AND(b, JAY_TYPE_U32, out, wx, jay_NOT_u32(b, wy)),
        jay_AND(b, JAY_TYPE_U32, out, wx, jay_negate(wy)));

   CASE(jay_XOR(b, JAY_TYPE_U32, out, jay_NOT_u32(b, wx), wy),
        jay_XOR(b, JAY_TYPE_U32, out, jay_negate(wx), wy));

   CASE(jay_OR(b, JAY_TYPE_U32, out, jay_NOT_u32(b, wx), jay_NOT_u32(b, wy)),
        jay_OR(b, JAY_TYPE_U32, out, jay_negate(wx), jay_negate(wy)));
}

TEST_F(Optimizer, NegativeFusedFneg)
{
   for (unsigned i = 0; i < ARRAY_SIZE(float_types); ++i) {
      enum jay_type T = float_types[i];
      NEGCASE(jay_ADD(b, JAY_TYPE_U32, out, wx, MOV(T, NEG(wy))));
      NEGCASE(jay_ADD(b, JAY_TYPE_S32, out, wx, MOV(T, NEG(wy))));
   }
}

/* TODO: test fneg with f64 */

TEST_F(Optimizer, FusedSat)
{
   for (unsigned i = 0; i < ARRAY_SIZE(float_types); ++i) {
      enum jay_type T = float_types[i];

      CASE(
         {
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_ADD(b, T, x, wx, MOV(T, NEG(wy)));
            jay_MODIFIER(b, T, out, x)->saturate = true;
         },
         { jay_ADD(b, T, out, wx, NEG(wy))->saturate = true; });

      CASE(
         {
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_MUL(b, T, x, wx, MOV(T, NEG(wy)));
            jay_MODIFIER(b, T, out, x)->saturate = true;
         },
         { jay_MUL(b, T, out, wx, NEG(wy))->saturate = true; });

      CASE(
         {
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_MAX(b, T, x, wx, MOV(T, NEG(wy)))->saturate = true;
            jay_MODIFIER(b, T, out, x)->saturate = true;
         },
         { jay_MAX(b, T, out, wx, NEG(wy))->saturate = true; });
   }
}

TEST_F(Optimizer, InverseBallotPropagate)
{
   CASE(
      {
         jay_def x = jay_alloc_def(b, UGPR, 1);
         jay_def f = jay_alloc_def(b, FLAG, 1);
         jay_ADD(b, JAY_TYPE_U32, x, wx, wy);
         jay_MOV(b, f, x);
         jay_SEL(b, JAY_TYPE_U32, out, wx, wy, f);
      },
      {
         UNUSED jay_def x = jay_alloc_def(b, UGPR, 1);
         jay_def f = jay_alloc_def(b, FLAG, 1);
         jay_ADD(b, JAY_TYPE_U32, f, wx, wy);
         jay_SEL(b, JAY_TYPE_U32, out, wx, wy, f);
      });
}

TEST_F(Optimizer, GtZero)
{
   CASE(
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_LT, flag, 0, x);
         jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
      },
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_inst *add = jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_set_conditional_mod(b, add, flag, JAY_CONDITIONAL_GT);
         jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
      });
}

TEST_F(Optimizer, MultipleCmp)
{
   CASE(
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def flag2 = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_LT, flag, 0, x);
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_GT, flag2, 0, x);
         jay_SEL(b, JAY_TYPE_U32, out, x, jay_SEL_u32(b, x, 123, flag), flag2);
      },
      {
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def flag2 = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_inst *add = jay_ADD(b, JAY_TYPE_S32, x, wx, NEG(wy));
         jay_set_conditional_mod(b, add, flag, JAY_CONDITIONAL_GT);
         jay_CMP(b, JAY_TYPE_S32, JAY_CONDITIONAL_GT, flag2, 0, x);
         jay_SEL(b, JAY_TYPE_U32, out, x, jay_SEL_u32(b, x, 123, flag), flag2);
      });
}

TEST_F(Optimizer, TypeNeutralConditionalMods)
{
   enum jay_conditional_mod mods[] = {
      JAY_CONDITIONAL_NE,
      JAY_CONDITIONAL_EQ,
   };

   for (unsigned i = 0; i < 2; ++i) {
      CASE(
         {
            jay_def flag = jay_alloc_def(b, FLAG, 1);
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_BFN(b, x, wx, wy, wz, UTIL_LUT3(a & b & c));
            jay_CMP(b, JAY_TYPE_S32, mods[i], flag, x, 0);
            jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
         },
         {
            jay_def flag = jay_alloc_def(b, FLAG, 1);
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_inst *bfn3 = jay_BFN(b, x, wx, wy, wz, UTIL_LUT3(a & b & c));

            /* BFN.ne is not permitted & should not be propagated */
            if (mods[i] == JAY_CONDITIONAL_EQ) {
               jay_set_conditional_mod(b, bfn3, flag, mods[i]);
            } else {
               jay_CMP(b, JAY_TYPE_S32, mods[i], flag, x, 0);
            }

            jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
         });

      CASE(
         {
            jay_def flag = jay_alloc_def(b, FLAG, 1);
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_AND(b, JAY_TYPE_U32, x, wx, wy);
            jay_CMP(b, JAY_TYPE_S32, mods[i], flag, x, 0);
            jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
         },
         {
            jay_def flag = jay_alloc_def(b, FLAG, 1);
            jay_def x = jay_alloc_def(b, GPR, 1);
            jay_inst *an = jay_AND(b, JAY_TYPE_U32, x, wx, wy);
            jay_set_conditional_mod(b, an, flag, mods[i]);
            jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
         });
   }
}

TEST_F(Optimizer, SignednessMismatchConditionalMods)
{
   enum jay_conditional_mod mods[] = {
      JAY_CONDITIONAL_LE,
      JAY_CONDITIONAL_GT,
   };

   for (unsigned i = 0; i < 2; ++i) {
      NEGCASE({
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_BFN(b, x, wx, wy, wz, UTIL_LUT3(a & b & c));
         jay_CMP(b, JAY_TYPE_S32, mods[i], flag, x, 0);
         jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
      });
   }
}

TEST_F(Optimizer, FloatMismatchConditionalMods)
{
   enum jay_conditional_mod mods[] = {
      JAY_CONDITIONAL_NAN,
      JAY_CONDITIONAL_EQ,
      JAY_CONDITIONAL_NE,
      JAY_CONDITIONAL_LT,
   };

   for (unsigned i = 0; i < 2; ++i) {
      NEGCASE({
         jay_def flag = jay_alloc_def(b, FLAG, 1);
         jay_def x = jay_alloc_def(b, GPR, 1);
         jay_BFN(b, x, wx, wy, wz, UTIL_LUT3(a & b & c));
         jay_CMP(b, JAY_TYPE_F32, mods[i], flag, x, 0);
         jay_SEL(b, JAY_TYPE_U32, out, x, 123, flag);
      });
   }
}
