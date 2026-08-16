/*
 * Copyright 2026 Intel Corporation
 * SPDX-License-Identifier: MIT
 */

#include <assert.h>
#include "util/bitscan.h"
#include "util/bitset.h"
#include "util/macros.h"
#include "util/ralloc.h"
#include "util/sparse_bitset.h"
#include "util/u_dynarray.h"
#include "util/u_math.h"
#include "jay_builder.h"
#include "jay_ir.h"
#include "jay_opcodes.h"
#include "jay_private.h"
#include "shader_enums.h"

/**
 * Register allocation for Jay shaders.
 *
 * We use a decoupled register allocation approach.  First, we spill values
 * until the register demand fits within the size of each register file.
 *
 * Secondly, we assign registers using a tree-scan algorithm similar to the
 * one described in Colombet et al 2011:
 *
 *    Q. Colombet, B. Boissinot, P. Brisk, S. Hack and F. Rastello,
 *        "Graph-coloring and treescan register allocation using repairing,"
 *        2011 Proceedings of the 14th International Conference on Compilers,
 *        Architectures and Synthesis for Embedded Systems (CASES), Taipei,
 *        Taiwan, 2011, pp. 45-54, doi: 10.1145/2038698.2038708.
 *
 * We also use a union-find set to construct equivalence classes for phi webs,
 * and attempt to use the same regs for registers in that class, similar to
 * the "Aggressive Pre-Coalescing" step described in that paper.
 *
 * Finally, we deconstruct SSA.
 */

static inline bool
is_ra_src(jay_def d)
{
   return d.file < JAY_NUM_RA_FILES;
}

#define jay_foreach_ra_file(file)                                              \
   for (enum jay_file file = 0; file < JAY_NUM_RA_FILES; ++file)

#define jay_foreach_ra_src(I, s)                                               \
   jay_foreach_src(I, s)                                                       \
      if (is_ra_src(I->src[s]) && !jay_is_null(I->src[s]))

static enum jay_stride
jay_min_stride_for_type(enum jay_type T)
{
   unsigned bits = jay_type_size_bits(T);

   /* We need at least enough contiguous bits per-lane to store a scalar */
   if (bits == 64)
      return JAY_STRIDE_8;
   else if (bits == 32)
      return JAY_STRIDE_4;
   else
      return JAY_STRIDE_2;
}

static enum jay_stride
jay_max_stride_for_type(enum jay_type T)
{
   /* Horizontal stride can be at most 4 */
   return (jay_type_size_bits(T) >= 16) ? JAY_STRIDE_8 : JAY_STRIDE_4;
}

static bool
jay_restrict_mixed_strides(jay_inst *I, unsigned s)
{
   /* From the hardware spec section "Register Region Restrictions":
    *
    * "In case of all floating point data types used in destination:" and
    *
    * "In case where source or destination datatype is 64b or operation is
    *  integer DWord multiply:" and
    *
    *  "Src2 Restrictions"
    *
    *      Register Regioning patterns where register data bit location
    *      of the LSB of the channels are changed between source and
    *      destination are not supported on Src0 and Src1 except for
    *      broadcast of a scalar.
    *
    * Therefore, ban mixed-strides in these cases.
    *
    * Similarly, SENDs cannot do any regioning so restrict that too.
    */
   return jay_type_is_any_float(I->type) ||
          jay_type_size_bits(I->type) == 64 ||
          jay_is_send_like(I) ||
          I->op == JAY_OPCODE_MUL_32X16 ||
          I->op == JAY_OPCODE_MUL_32 ||
          s == 2;
}

static enum jay_stride
jay_dst_stride_minmax(jay_inst *I, bool do_max)
{
   enum jay_stride min = jay_min_stride_for_type(I->type);
   enum jay_stride max = jay_max_stride_for_type(I->type);

   /* Destination stride must be equal to the ratio of the sizes of the
    * execution data type to the destination type
    */
   if (I->op == JAY_OPCODE_CVT) {
      min = MAX2(min, jay_min_stride_for_type(jay_src_type(I, 0)));
   }

   /* V/UV types are restricted */
   if (I->op == JAY_OPCODE_SHR_ODD_SUBSPANS_BY_4) {
      return JAY_STRIDE_2;
   }

   /* The src2 restriction quoted above effectively implies we should not stride
    * destinations of 3-source instructions either.
    */
   if (jay_num_isa_srcs(I) >= 3) {
      return min;
   }

   return (do_max && !jay_restrict_mixed_strides(I, 0)) ? max : min;
}

static enum jay_stride
jay_src_stride_minmax(jay_inst *I, unsigned s, bool do_max)
{
   enum jay_stride min = jay_min_stride_for_type(jay_src_type(I, s));
   enum jay_stride max = jay_max_stride_for_type(jay_src_type(I, s));

   /* SENDs cannot do any regioning so force exactly the types of the sources
    * regardless of the type of the destination.
    *
    * Shuffles could theoretically support regioning but it would be nontrivial
    * and probably pointless most of the time.
    */
   if (jay_is_send_like(I) || jay_is_shuffle_like(I)) {
      return min;
   }

   /* While "add.u16 r0<2>, r1<4>" is legal, "add.u16 r0, r1<4>" is not.
    * Conservatively assume the destination is packed and restrict the source
    * stride accordingly. This satisfies the special restrictions.
    */
   if (jay_type_size_bits(I->type) <= 16) {
      max = JAY_STRIDE_4;
   }

   /* "add.u16 r0.8, g1<2>" is not legal. We don't generate this normally yet
    * (preferring to burn the upper bits) but it is used internally.
    */
   if (I->op == JAY_OPCODE_LANE_ID_EXPAND) {
      max = JAY_STRIDE_2;
   }

   if (jay_restrict_mixed_strides(I, s) &&
       jay_type_size_bits(jay_src_type(I, s)) < jay_type_size_bits(I->type)) {

      return jay_dst_stride_minmax(I, do_max);
   }

   return (do_max && !jay_restrict_mixed_strides(I, s)) ? max : min;
}

struct affinity {
   /**
    * If there is a vector affinity defined for this SSA def, it is relative to
    * some representative SSA index. Else 0 if there is no affinity.
    */
   uint32_t repr;

   /** If the representative: offset in registers from the base.
    *
    * If not the representative: offset in registers from the representative. */
   signed offset:4;

   /**
    * If true, this value is used in an end-of-thread SEND and requires high
    * registers.
    */
   bool eot:1;

   /** If true, this UGPR needs full GRF alignment */
   unsigned align     :5;
   unsigned align_offs:4;
   unsigned padding   :18;
};
static_assert(sizeof(struct affinity) == 8, "packed");

struct phi_web_node {
   /* Parent index, or circular for root */
   uint32_t parent;

   /* If root, assigned register, or ~0 if no register assigned. */
   uint16_t reg;

   /* Rank, at most log2(n) so need ~5-bits */
   uint16_t rank;

   /* If root, affinity for the whole web */
   struct affinity affinity;
};
static_assert(sizeof(struct phi_web_node) == 16, "packed");

static unsigned
phi_web_find(struct phi_web_node *web, unsigned x)
{
   if (web[x].parent == x) {
      /* Root */
      return x;
   } else {
      /* Search up the tree */
      unsigned root = x;
      while (web[root].parent != root)
         root = web[root].parent;

      /* Compress path. Second pass ensures O(1) memory usage. */
      while (web[x].parent != x) {
         unsigned temp = web[x].parent;
         web[x].parent = root;
         x = temp;
      }

      return root;
   }
}

static void
phi_web_union(struct phi_web_node *web, unsigned x, unsigned y)
{
   x = phi_web_find(web, x);
   y = phi_web_find(web, y);

   if (x == y)
      return;

   /* Union-by-rank: ensure x.rank >= y.rank */
   if (web[x].rank < web[y].rank) {
      SWAP(x, y);
   }

   web[y].parent = x;

   /* Increment rank if necessary */
   if (web[x].rank == web[y].rank) {
      web[x].rank++;
   }
}

#define NO_REG 0xFFFF

static inline jay_reg
make_reg(enum jay_file file, uint16_t reg)
{
   return (((uint16_t) file) << 13) | reg;
}

static inline unsigned
r_reg(jay_reg r)
{
   assert(r != NO_REG);
   return r & BITFIELD_MASK(13);
}

static inline enum jay_file
r_file(jay_reg r)
{
   assert(r != NO_REG);
   assert((r >> 13) < JAY_NUM_RA_FILES);
   return r >> 13;
}

static jay_def
def_from_reg(jay_reg r)
{
   return jay_bare_reg(r_file(r), r_reg(r));
}

typedef struct jay_ra_state {
   /** Size of each register file */
   unsigned num_regs[JAY_NUM_RA_FILES];

   /** Counter for roundrobin register allocation */
   unsigned roundrobin[JAY_NUM_RA_FILES];

   /** First GPR that may be used for EOT sends */
   unsigned eot_offs;

   /** Phi coalescing data structure */
   struct phi_web_node *phi_web;

   /**
    * Global SSA index -> jay_reg map. Unlike reg_for_index, once a register
    * is picked it will not be shuffled.
    */
   jay_reg *global_reg_for_index;

   /**
    * Block currently being processed. ra_state is allocated once per
    * function but the following fields are updated as we go through the
    * program. This keeps RA linearish time.
    */
   jay_block *block;

   /** Builder for inserting shuffle code */
   jay_builder bld;

   /** Local SSA index -> jay_reg map. Only defined for live indices. */
   jay_reg *reg_for_index;

   /**
    * Value occupying a register (register -> uint32_t reverse maps) for
    * registers that are not available. Undefined for available registers.
    */
   uint32_t *index_for_reg[JAY_NUM_RA_FILES];

   /** Set of registers that are available */
   BITSET_WORD *available_regs[JAY_NUM_RA_FILES];

   /**
    * Within assign_regs_for_inst, the set of registers that have been
    * assigned and are therefore pinned.
    *
    * Invariant: zeroed on entry to assign_regs_for_inst.
    */
   BITSET_WORD *pinned[JAY_NUM_RA_FILES];

   /** Vector affinities for each def. */
   struct affinity *affinities;
} jay_ra_state;

static inline jay_reg
current_reg(const jay_ra_state *ra, uint32_t index)
{
   assert(index > 0 && index < ra->bld.func->ssa_alloc);
   jay_reg reg = ra->reg_for_index[index];

   assert(reg != NO_REG);
   assert(ra->index_for_reg[r_file(reg)][r_reg(reg)] == index);
   return reg;
}

/** (dst, src) pairs for use in parallel copies */
struct jay_parallel_copy {
   jay_reg dst, src;
};

static void
add_copy(struct util_dynarray *copies, jay_reg dst, jay_reg src)
{
   if (dst != src) {
      assert(r_file(dst) == r_file(src));
      util_dynarray_append(copies, ((struct jay_parallel_copy) { dst, src }));
   }
}

static jay_def
push_temp(jay_builder *b, jay_reg reg, bool stride4)
{
   jay_def tmp = def_from_reg(reg);

   if (stride4 && jay_def_stride(b->shader, tmp) != JAY_STRIDE_4) {
      jay_def new = def_from_reg(0);
      jay_MOV(b, tmp, new);
      tmp = new;
   }

   return tmp;
}

static void
pop_temp(jay_builder *b, struct jay_temp_regs t, jay_def temp)
{
   if (temp.file == GPR && temp.reg != t.gpr) {
      jay_MOV(b, temp, def_from_reg(t.gpr));
   }
}

/*
 * Insert a single logical copy. Like jay_MOV but expands to multiple moves
 * involving a temporary register in some cases.
 */
static void
mov(jay_builder *b, jay_def dst, jay_def src, struct jay_temp_regs temps)
{
   if (dst.file == MEM && src.file == MEM) {
      assert(temps.gpr != NO_REG && "ensured by the spill limit");
      jay_def temp = push_temp(b, temps.gpr, true /* stride4 */);
      jay_MOV(b, temp, src);
      jay_MOV(b, dst, temp);
      pop_temp(b, temps, temp);
   } else if (dst.file == UMEM && src.file == UMEM) {
      assert(temps.ugpr != NO_REG && "ensured by the spill limit");
      jay_MOV(b, def_from_reg(temps.ugpr), src);
      jay_MOV(b, dst, def_from_reg(temps.ugpr));
   } else {
      jay_MOV(b, dst, src);
   }
}

/*
 * Sequentialize a parallel copy. temps are registers free *before* the
 * parallel copy. A temporary might be the destination of a copy, but it
 * cannot be the source of any copy (since copying a free register is
 * undefined). Therefore it cannot be a part of a cycle, so it is free for use
 * (only) when handling cycles, which must happen before sequential copies.
 */
static void
jay_emit_parallel_copies(jay_builder *b,
                         struct jay_parallel_copy *pcopies,
                         unsigned num_copies,
                         struct jay_temp_regs temps)
{
   /* Compact away trivial copies upfront to reduce runtime. */
   unsigned new_num_copies = 0;
   for (unsigned i = 0; i < num_copies; ++i) {
      assert(r_file(pcopies[i].dst) == r_file(pcopies[i].src));

      if (pcopies[i].dst != pcopies[i].src) {
         pcopies[new_num_copies++] = pcopies[i];
      }
   }

   num_copies = new_num_copies;
   if (num_copies == 0)
      return;

   assert(num_copies < UINT16_MAX);
   BITSET_WORD *done = BITSET_CALLOC(num_copies);
   uint16_t *reg_use_count[JAY_NUM_RA_FILES];
   jay_foreach_ra_file(f) {
      reg_use_count[f] = calloc(b->shader->num_regs[f], sizeof(uint16_t));
   }

   struct jay_parallel_copy *simple = malloc(num_copies * sizeof(*simple));
   unsigned num_simple = 0;

#ifndef NDEBUG
   BITSET_WORD *packed = BITSET_CALLOC(UINT16_MAX);

   if (0) {
      const char *files = "ruMm";
      printf("[[\n");

      for (unsigned i = 0; i < num_copies; i++) {
         printf("  %c%u = %c%u\n", files[r_file(pcopies[i].dst)],
                r_reg(pcopies[i].dst), files[r_file(pcopies[i].src)],
                r_reg(pcopies[i].src));
      }

      printf("]]\n");
   }

   /**
    * Assert that each parallel copy destination is unique: no reg can appear
    * as the destination of two parallel copies.
    */
   for (unsigned i = 0; i < num_copies; i++) {
      assert(!BITSET_TEST(packed, pcopies[i].dst));
      BITSET_SET(packed, pcopies[i].dst);
   }

   free(packed);
#endif

   for (unsigned i = 0; i < num_copies; i++) {
      ++reg_use_count[r_file(pcopies[i].src)][r_reg(pcopies[i].src)];
   }

   bool progress;
   do {
      progress = false;

      /* Step 1: resolve paths in the transfer graph. This means finding
       * copies whose destination aren't blocked by something else and then
       * emitting them, continuing this process until every copy is blocked
       * and there are only cycles left.
       *
       * TODO: We should note that src is also available in dest to unblock
       * cycles that src is involved in.
       */
      for (unsigned i = 0; i < num_copies; i++) {
         struct jay_parallel_copy *copy = &pcopies[i];

         if (!BITSET_TEST(done, i) &&
             reg_use_count[r_file(copy->dst)][r_reg(copy->dst)] == 0) {

            simple[num_simple++] = *copy;
            BITSET_SET(done, i);
            --reg_use_count[r_file(copy->src)][r_reg(copy->src)];
            progress = true;
         }
      }
   } while (progress);

   /* Step 2: resolve cycles through swapping.
    *
    * At this point, the transfer graph should consist of only cycles.
    * The reason is that, given any reg n_1 that's the source of a
    * remaining entry, it has a destination n_2, which (because every
    * copy is blocked) is the source of some other copy whose destination
    * is n_3, and so we can follow the chain until we get a cycle. If we
    * reached some other node than n_1:
    *
    * n_1 -> n_2 -> ... -> n_i
    *         ^             |
    *         |-------------|
    *
    * then n_2 would be the destination of 2 copies, which is illegal
    * (checked above in an assert). So n_1 must be part of a cycle:
    *
    * n_1 -> n_2 -> ... -> n_i
    * ^                     |
    * |---------------------|
    *
    * and this must be only cycle n_1 is involved in, because any other
    * path starting from n_1 would also have to end in n_1, resulting in
    * a node somewhere along the way being the destination of 2 copies
    * when the 2 paths merge.
    *
    * The way we resolve the cycle is through picking a copy (n_1, n_2)
    * and swapping n_1 and n_2. This moves n_1 to n_2, so n_2 is taken
    * out of the cycle:
    *
    * n_1 -> ... -> n_i
    * ^              |
    * |--------------|
    *
    * and we can keep repeating this until the cycle is empty. After each
    * swap, we update sources of blocking copies. At that point, every
    * blocking copy's source should be contained within our destination.
    */
   for (unsigned i = 0; i < num_copies; i++) {
      struct jay_parallel_copy *copy = &pcopies[i];

      if (!BITSET_TEST(done, i) && copy->dst != copy->src) {
         jay_def dst = def_from_reg(copy->dst), src = def_from_reg(copy->src);
         assert(dst.file == src.file);
         enum jay_file file = dst.file;
         jay_reg tmp = (file == GPR || file == MEM) ? temps.gpr : temps.ugpr;

         if (tmp != NO_REG) {
            struct jay_temp_regs t = { .gpr = temps.gpr2, .ugpr = temps.ugpr2 };
            jay_def temp = push_temp(b, tmp, file == MEM /* stride4 */);
            {
               mov(b, temp, dst, t);
               mov(b, dst, src, t);
               mov(b, src, temp, t);
            }
            pop_temp(b, temps, temp);
         } else {
            jay_SWAP(b, dst, src);
         }

         for (unsigned j = 0; j < num_copies; j++) {
            if (pcopies[j].src == copy->dst)
               pcopies[j].src = copy->src;
         }

         /* Simple copies are deferred. Their destinations do not conflict with
          * our swaps, but we need to swap their sources to sink.
          */
         for (unsigned j = 0; j < num_simple; j++) {
            assert(simple[j].dst != copy->src && simple[j].dst != copy->dst);

            if (simple[j].src == copy->src)
               simple[j].src = copy->dst;
            else if (simple[j].src == copy->dst)
               simple[j].src = copy->src;
         }
      }

      BITSET_SET(done, i);
   }

   /* Emit moves after swaps because they fan out and thus increase demand.
    * This gives us more freedom around temporaries. The rewrite of simple
    * copies above ensures correctness.
    *
    * Simiarly, we first emit memory-memory copies since those require
    * temporaries but only register copies can clobber the temporaries.
    */
   for (unsigned i = 0; i < num_simple; i++) {
      jay_def dst = def_from_reg(simple[i].dst);
      jay_def src = def_from_reg(simple[i].src);

      if (jay_is_mem(dst) && jay_is_mem(src)) {
         mov(b, dst, src, temps);
      }
   }

   for (unsigned i = 0; i < num_simple; i++) {
      jay_def dst = def_from_reg(simple[i].dst);
      jay_def src = def_from_reg(simple[i].src);

      if (!(jay_is_mem(dst) && jay_is_mem(src))) {
         mov(b, dst, src, temps);

         if (temps.gpr == simple[i].dst || temps.gpr == simple[i].src) {
            temps.gpr = NO_REG;
         }

         if (temps.ugpr == simple[i].dst || temps.ugpr == simple[i].src) {
            temps.ugpr = NO_REG;
         }
      }
   }

   jay_foreach_ra_file(f) {
      free(reg_use_count[f]);
   }

   free(simple);
   free(done);
}

static bool
reg_is_available(jay_ra_state *ra, jay_reg reg)
{
   assert(reg != NO_REG);
   return BITSET_TEST(ra->available_regs[r_file(reg)], r_reg(reg));
}

static void
assign_reg_for_index(jay_ra_state *ra, uint32_t index, jay_reg reg)
{
   /* Update our data structures */
   ra->reg_for_index[index] = reg;
   ra->index_for_reg[r_file(reg)][r_reg(reg)] = index;
   BITSET_CLEAR(ra->available_regs[r_file(reg)], r_reg(reg));

   /* Update the web to the most recent register. Heuristic from Colombet. */
   ra->phi_web[phi_web_find(ra->phi_web, index)].reg = reg;

   /* Post-conditions */
   assert(!reg_is_available(ra, reg));
   assert(current_reg(ra, index) == reg);
}

static void
release_reg(jay_ra_state *ra, jay_reg reg)
{
   /* Update available_regs only - the reg<-->index maps are invalidated. */
   BITSET_SET(ra->available_regs[r_file(reg)], r_reg(reg));
}

static unsigned
register_demand(jay_ra_state *ra, enum jay_file f)
{
   unsigned n = ra->num_regs[f];
   return n - __bitset_prefix_sum(ra->available_regs[f], n, BITSET_WORDS(n));
}

static jay_reg
try_find_free_reg(jay_ra_state *ra, enum jay_file file, unsigned except)
{
   unsigned i;

   /* Prefer stride 4 temporaries, since they are more compatible and thus
    * should reduce swapping on average.
    */
   if (file == GPR) {
      BITSET_FOREACH_SET(i, ra->available_regs[file], ra->num_regs[file]) {
         if (i != except &&
             jay_gpr_to_stride(&ra->bld.shader->partition, i) == JAY_STRIDE_4) {
            return make_reg(file, i);
         }
      }
   }

   BITSET_FOREACH_SET(i, ra->available_regs[file], ra->num_regs[file]) {
      if (i != except) {
         return make_reg(file, i);
      }
   }

   return NO_REG;
}

static jay_reg
find_free_reg(jay_ra_state *ra, enum jay_file file, unsigned except)
{
   jay_reg reg = try_find_free_reg(ra, file, except);

   if (reg == NO_REG) {
      fprintf(stderr, "file %u, current demand %u, target %u\n", file,
              register_demand(ra, file), ra->num_regs[file]);
      UNREACHABLE("there should have been a free register");
   }

   return reg;
}

static inline struct jay_temp_regs
find_temp_regs(jay_ra_state *ra)
{
   jay_reg gpr = try_find_free_reg(ra, GPR, ~0);
   jay_reg ugpr = try_find_free_reg(ra, UGPR, ~0);

   return (struct jay_temp_regs) {
      .gpr = gpr,
      .ugpr = ugpr,
      .gpr2 = try_find_free_reg(ra, GPR, gpr),
      .ugpr2 = try_find_free_reg(ra, UGPR, ugpr),
   };
}

static unsigned
pick_regs(jay_ra_state *ra,
          enum jay_file file,
          unsigned size,
          unsigned alignment,
          enum jay_stride min_stride,
          enum jay_stride max_stride,
          jay_inst *I,
          jay_def var,
          jay_def *last_killed,
          bool is_src)
{
   struct jay_partition *partition = &ra->bld.shader->partition;
   unsigned first = 0, end = ra->num_regs[file];
   bool must_tie = I->op == JAY_OPCODE_LANE_ID_EXPAND;
   must_tie &= !is_src;

   /* Cross-lane access cannot be SIMD split if the source/destination registers
    * overlap, but as long as we don't tie those destinations, we're ok.
    */
   bool may_tie = !jay_is_shuffle_like(I);

   /* Ensure we do not cross partitions */
   if (file == UGPR && size > 16) {
      first = partition->large_ugpr_block.start;
      end = partition->large_ugpr_block.start + partition->large_ugpr_block.len;
   }

   /* Sources used by end-of-thread sends must be at the end of the file */
   if (I->op == JAY_OPCODE_SEND && jay_send_eot(I)) {
      first = ra->eot_offs;
   }

   /* If possible, keep sources in place to avoid shuffles. */
   if (is_src && jay_channel(var, 0) != 0) {
      unsigned cur = r_reg(ra->reg_for_index[jay_channel(var, 0)]);
      enum jay_stride stride = jay_gpr_to_stride(partition, cur);

      if (!BITSET_TEST_COUNT(ra->pinned[file], cur, size) &&
          util_is_aligned(cur, alignment) &&
          cur >= first &&
          cur + size <= end &&
          (file != GPR || (min_stride <= stride && stride <= max_stride))) {
         return cur;
      }
   }

   unsigned best_cost = UINT32_MAX;
   unsigned best_reg = 0;
   struct affinity affinity =
      ra->phi_web[phi_web_find(ra->phi_web, jay_channel(var, 0))].affinity;

   assert(alignment >= size && "alignment must be a multiple of size");
   unsigned nr = DIV_ROUND_UP((end + 1 - size - first), alignment);
   unsigned roundrobin = (ra->roundrobin[file]) % nr;
   unsigned rr_al = roundrobin * alignment, nr_al = nr * alignment;
   ra->roundrobin[file] += size;

   for (unsigned i = rr_al; i < rr_al + nr_al; i += alignment) {
      /* We select registers roundrobin. This has several benefits:
       *
       * 1. Easier coalescing since we are less likely statistically to allocate
       *    a register that a future instruction has an affinity.
       *
       * 2. More freedom for post-RA scheduling thanks to fewer dependencies.
       *
       * 3. Less stalling due to SWSB annotations from register reuse.
       */
      unsigned r = first + (i >= nr_al ? (i - nr_al) : i);
      assert(r >= first && r + size <= end);

      unsigned cost = 0;
      bool tied = last_killed && last_killed->reg == r;
      enum jay_stride stride =
         file == GPR ? jay_gpr_to_stride(partition, r) : min_stride;

      if ((tied ? !may_tie :
                  (must_tie || BITSET_TEST_COUNT(ra->pinned[file], r, size))) ||
          !(min_stride <= stride && stride <= max_stride))
         continue;

      /* Assigning a stride that is too big may result in SIMDness splitting.
       * Model that cost so we prefer packed registers.
       */
      cost += stride - min_stride;

      /* If we are used for end-of-thread and it is not in the appropriate
       * register, we will need to insert 1 copy per channel at the end.
       */
      if (affinity.eot && r < ra->eot_offs)
         cost += size;

      /* If there are stricter alignment requirements later, model the cost of
       * inserting copies for that.
       */
      if (affinity.align &&
          !util_is_aligned(r - affinity.align_offs, affinity.align))
         cost += size;

      if (affinity.repr == jay_channel(var, 0)) {
         /* If we are the collect representative but the final collect won't
          * actually be usable, the whole vector will need to be copied.
          */
         if (!util_is_aligned(r - affinity.offset, 8) ||
             (affinity.eot && r - affinity.offset < ra->eot_offs)) {
            cost += 8;
         }
      } else if (affinity.repr) {
         /* If we are used for a collect but not in the right place, we will
          * similarly insert copies.
          */
         if (ra->reg_for_index[affinity.repr] != NO_REG &&
             r_reg(ra->reg_for_index[affinity.repr]) != r - affinity.offset) {

            cost += size;
         }
      }

      for (unsigned c = 0; c < size; ++c) {
         unsigned i = r + c;

         /* If the register is unavailable, account for the cost of shuffling */
         if (!BITSET_TEST(ra->available_regs[file], i) && !tied) {
            cost++;

            /* ..plus the cost of shuffling back. */
            if (u_sparse_bitset_test(&ra->block->live_out,
                                     ra->index_for_reg[file][i]))
               cost++;
         }

         /* Model the cost of shuffling for phis */
         if (c < jay_num_values(var)) {
            struct phi_web_node *phi_web =
               &ra->phi_web[phi_web_find(ra->phi_web, jay_channel(var, c))];
            if (phi_web->reg != NO_REG && r_reg(phi_web->reg) != i) {
               cost += 2;
            }
         }

         /* Choosing this register will pin it, leaving it unavailable to later
          * smaller sources which will need to be shuffled. Account for those
          * moves.
          *
          * TODO: Faster algorithm.
          */
         jay_foreach_src_index(I, s, c, index) {
            if (jay_num_values(I->src[s]) < size &&
                ra->reg_for_index[index] == make_reg(file, i)) {
               cost++;
            }
         }
      }

      if (cost < best_cost) {
         best_cost = cost;
         best_reg = r;

         /* If we find something with 0 cost, we are guaranteed to pick this
          * register, so terminate early. This speeds up the search.
          */
         if (cost == 0) {
            break;
         }
      }
   }

   assert(best_cost != UINT32_MAX && "we always find something");
   assert(best_reg + size <= ra->num_regs[file]);
   return best_reg;
}

struct window {
   jay_reg base;
   uint16_t length;
};
static_assert(sizeof(struct window) == 4, "packed");

static void
assign_regs_for_inst(jay_ra_state *ra, jay_inst *I)
{
   jay_shader *shader = ra->bld.shader;
   jay_def *vars[JAY_MAX_OPERANDS];
   jay_def *last_killed[JAY_NUM_RA_FILES] = { 0 };
   jay_def saved_srcs[JAY_MAX_SRCS];
   struct jay_parallel_copy copies[JAY_MAX_DEF_LENGTH * JAY_MAX_OPERANDS];
   uint32_t eviction_indices[JAY_MAX_DEF_LENGTH * JAY_MAX_OPERANDS];
   unsigned nr_vars = 0, nr_copies = 0;

   /* Gather temporary registers that are free /before/ any shuffling */
   struct jay_temp_regs temp_regs = find_temp_regs(ra);

   /* Save sources so we can get at last-use info even after munging */
   typed_memcpy(saved_srcs, I->src, I->num_srcs);

   /* Gather sources (in order) then destinations. This order (with a stable
    * sort) ensures we see killed sources before same-size destinations,
    * naturally tying the last source to the destination. Predicated default
    * values rely on this invariant for correctness.
    */
   jay_foreach_ra_src(I, s) {
      /* Filter out duplicate scalar sources - they should only be assigned
       * once. Duplicated vector sources are lowered away as a precondition.
       */
      bool duplicate = false;
      if (jay_num_values(I->src[s]) == 1) {
         uint32_t index = jay_index(I->src[s]);

         for (unsigned i = 0; i < nr_vars; ++i) {
            jay_def var = *(vars[i]);
            duplicate |= (jay_num_values(var) == 1 && jay_index(var) == index);
         }
      }

      if (!duplicate) {
         vars[nr_vars++] = &I->src[s];

         /* Record the old registers as parallel copies to be filled in later.
          * Then release the old registers to be reassigned.
          */
         jay_foreach_index(I->src[s], _, index) {
            jay_reg reg = current_reg(ra, index);
            assert(reg != NO_REG);

            eviction_indices[nr_copies] = index;
            copies[nr_copies++] = (struct jay_parallel_copy) { .src = reg };
            release_reg(ra, reg);
         }
      }
   }

   if (!jay_is_null(I->dst) && I->dst.file < JAY_NUM_RA_FILES) {
      vars[nr_vars++] = &I->dst;
   }

   /* Sort variables by size in descending order. We use insertion sort
    * because it is stable, adaptive, and faster than mergesort for small n.
    *
    * Algorithm from CLRS.
    */
   for (unsigned i = 1; i < nr_vars; ++i) {
      jay_def *pivot = vars[i];
      unsigned j, key = pivot->num_values_m1;

      for (j = i; j > 0 && key > vars[j - 1]->num_values_m1; --j) {
         vars[j] = vars[j - 1];
      }

      vars[j] = pivot;
   }

   /* Partition `copies` into "source shuffles" and "livethrough shuffles" */
   uint32_t first_eviction_copy = nr_copies;

   /* Choose registers for sources/destinations in order */
   for (unsigned i = 0; i < nr_vars; ++i) {
      bool is_src = vars[i] >= I->src;
      bool killed = false;
      jay_def var = *(vars[i]);
      unsigned size = jay_num_values(var);
      if (is_src) {
         assert(util_is_power_of_two_nonzero(size) && "NPOT sources lowered");
      } else {
         size = util_next_power_of_two(size);
      }

      unsigned alignment = I->op == JAY_OPCODE_EXPAND_QUAD ? 1 : size;
      enum jay_file file = var.file;
      enum jay_stride min_stride = JAY_STRIDE_2, max_stride = JAY_STRIDE_8;

      assert(size > 0 && file < JAY_NUM_RA_FILES && "filtered above");

      if (is_src) {
         /* If a source is duplicated, we need to take the most constrained
          * version. This matters for 3-src restrictions.
          */
         jay_foreach_src(I, s) {
            if (jay_defs_equivalent(var, I->src[s])) {
               alignment = MAX2(alignment, jay_src_alignment(shader, I, s));
               min_stride =
                  MAX2(jay_src_stride_minmax(I, s, false), min_stride);
               max_stride = MIN2(jay_src_stride_minmax(I, s, true), max_stride);
            }
         }

         unsigned s = vars[i] - I->src;

         /* Sources are considered killed only if completely killed */
         unsigned lu = jay_source_last_use_bit(saved_srcs, s);

         killed = true;
         for (unsigned i = 0; i < size; ++i) {
            if (jay_channel(I->src[s], i) == 0 ||
                !BITSET_TEST(I->last_use, lu + i)) {
               killed = false;
               break;
            }
         }
      } else {
         alignment = MAX2(alignment, jay_dst_alignment(shader, I));
         min_stride = jay_dst_stride_minmax(I, false);
         max_stride = jay_dst_stride_minmax(I, true);
      }

      /* Choose registers satisfying the constraints and minimizing shuffles */
      unsigned base =
         pick_regs(ra, file, size, alignment, min_stride, max_stride, I, var,
                   is_src ? NULL : last_killed[file], is_src);
      jay_reg reg = make_reg(file, base);

      /* If we decided to tie, process that */
      if (!is_src && last_killed[file] && last_killed[file]->reg == base) {
         /* Fully killed source so we can zero a contiguous range. Note we need
          * to use the unpadded size to avoid leaking a register for vec3
          * destinations tied to vec4 sources.
          */
         unsigned offs =
            jay_source_last_use_bit(saved_srcs, last_killed[file] - I->src);
         BITSET_CLEAR_COUNT(I->last_use, offs, jay_num_values(var));
         last_killed[file] = NULL;
      } else {
         /* Otherwise pin our choice */
         BITSET_SET_COUNT(ra->pinned[file], base, size);

         for (unsigned c = 0; c < size; ++c) {
            /* Evict any livethrough value interfering with our choice */
            if (!(is_src && jay_channel(var, c) == 0) &&
                !reg_is_available(ra, reg + c)) {
               uint32_t index = ra->index_for_reg[file][base + c];
               struct jay_parallel_copy copy = { .src = reg + c };
               eviction_indices[nr_copies] = index;
               copies[nr_copies++] = copy;
               release_reg(ra, reg + c);
            }
         }
      }

      jay_set_reg(vars[i], base);

      jay_foreach_index(var, c, index) {
         assign_reg_for_index(ra, index, reg + c);
      }

      if (killed) {
         last_killed[file] = vars[i];
      }
   }

   /* Set .reg late so duplicated scalar sources are handled properly */
   jay_foreach_ra_src(I, s) {
      if (I->src[s]._payload != JAY_SENTINEL) {
         jay_set_reg(&I->src[s],
                     r_reg(ra->reg_for_index[jay_channel(I->src[s], 0)]));
      }
   }

   /* Look up where shuffled sources ended up */
   for (unsigned i = 0; i < first_eviction_copy; ++i) {
      copies[i].dst = ra->reg_for_index[eviction_indices[i]];
   }

   /* Assign new registers for evicted values */
   for (unsigned i = first_eviction_copy; i < nr_copies; ++i) {
      copies[i].dst = find_free_reg(ra, r_file(copies[i].src), ~0);
      assign_reg_for_index(ra, eviction_indices[i], copies[i].dst);
   }

   /* Shuffle everything */
   ra->bld.cursor = jay_before_inst(I);
   jay_emit_parallel_copies(&ra->bld, copies, nr_copies, temp_regs);

   /* Reset data structures */
   for (unsigned i = 0; i < nr_vars; ++i) {
      jay_def var = *(vars[i]);
      BITSET_CLEAR_COUNT(ra->pinned[var.file], var.reg,
                         util_next_power_of_two(jay_num_values(var)));
   }

   /* Sources selected for early-kill have had their last_use fields cleared.
    * Anything else is late-killed. Release those registers.
    */
   unsigned kill_idx = 0;
   jay_foreach_ssa_src(I, s) {
      jay_foreach_index(saved_srcs[s], c, idx) {
         if (is_ra_src(I->src[s]) && BITSET_TEST(I->last_use, kill_idx)) {
            release_reg(ra, make_reg(I->src[s].file, I->src[s].reg + c));
         }

         kill_idx++;
      }
   }
}

static void
local_ra(jay_ra_state *ra, jay_block *block)
{
   ra->block = block;

   /* Initialize local data structures based on global state */
   jay_foreach_ra_file(file) {
      BITSET_SET_COUNT(ra->available_regs[file], 0, ra->num_regs[file]);
   }

   U_SPARSE_BITSET_FOREACH_SET(&block->live_in, i) {
      if (ra->global_reg_for_index[i] != NO_REG) {
         assign_reg_for_index(ra, i, ra->global_reg_for_index[i]);
      }
   }

   /* Assign registers locally */
   jay_foreach_inst_in_block(block, I) {
      if (I->op == JAY_OPCODE_PHI_SRC) {
         break;
      } else if (I->op == JAY_OPCODE_PHI_DST) {
         /* Phis are special as we never shuffle them */
         unsigned index = jay_index(I->dst);
         jay_reg reg = ra->phi_web[phi_web_find(ra->phi_web, index)].reg;

         if (reg == NO_REG || !reg_is_available(ra, reg)) {
            reg = find_free_reg(ra, I->dst.file, ~0);
         }

         assign_reg_for_index(ra, jay_index(I->dst), reg);
         I->dst.reg = r_reg(reg);
      } else if (I->op == JAY_OPCODE_PRELOAD) {
         /* Preloads always get what they want */
         I->dst.reg = jay_preload_reg(I);
         jay_reg base = make_reg(I->dst.file, I->dst.reg);

         jay_foreach_comp(I->dst, c) {
            assert(reg_is_available(ra, base + c) && "preloads always work");
            assign_reg_for_index(ra, jay_channel(I->dst, c), base + c);
         }
      } else {
         /* For normal instructions, assign registers. */
         assign_regs_for_inst(ra, I);
      }

      /* Release registers for destinations that are immediately killed */
      jay_foreach_index(I->dst, _, index) {
         if (BITSET_TEST(ra->bld.func->dead_defs, index)) {
            release_reg(ra, current_reg(ra, index));
         }
      }

      if (jay_debug & JAY_DBG_PRINTDEMAND) {
         printf("(RA) [G:%u\tU:%u] ", register_demand(ra, GPR),
                register_demand(ra, UGPR));
         jay_print_inst(stdout, I);
      }
   }

   /* Gather temporary registers that are free /before/ any shuffling */
   struct jay_temp_regs temp_regs = find_temp_regs(ra);

   /* Reconcile local state with the global structures */
   jay_foreach_ra_file(file) {
      BITSET_SET_COUNT(ra->available_regs[file], 0, ra->num_regs[file]);
   }

   /* Extend live ranges for correctness. Might be a better solution though. */
   jay_foreach_inst_in_block_rev(block, I) {
      if (I->op != JAY_OPCODE_PHI_SRC && !jay_op_is_control_flow(I->op)) {
         break;
      }

      jay_foreach_ra_src(I, s) {
         u_sparse_bitset_set(&block->live_out, jay_index(I->src[s]));
      }
   }

   /* Already assigned global registers need to be shuffled back */
   struct util_dynarray copies = UTIL_DYNARRAY_INIT;

   U_SPARSE_BITSET_FOREACH_SET(&block->live_out, i) {
      jay_reg lreg = ra->reg_for_index[i], greg = ra->global_reg_for_index[i];

      if (lreg != NO_REG && greg != NO_REG) {
         add_copy(&copies, greg, lreg);
         assign_reg_for_index(ra, i, greg);
      }
   }

   /* Live-out variables defined in this block need global registers assigned */
   U_SPARSE_BITSET_FOREACH_SET(&block->live_out, i) {
      jay_reg reg = ra->reg_for_index[i];

      if (ra->global_reg_for_index[i] == NO_REG && reg != NO_REG) {
         if (!reg_is_available(ra, reg)) {
            jay_reg old = reg;
            reg = find_free_reg(ra, r_file(reg), ~0);
            add_copy(&copies, reg, old);
         }

         assign_reg_for_index(ra, i, reg);
         ra->global_reg_for_index[i] = reg;
      }
   }

   /* Gather temporary registers free after shuffling (before phis) */
   block->temps_out = find_temp_regs(ra);

   /* Handle the end of the block */
   ra->bld.cursor = jay_before_block(block);

   jay_foreach_inst_in_block_rev(block, I) {
      if (I->op != JAY_OPCODE_PHI_SRC && !jay_op_is_control_flow(I->op)) {
         ra->bld.cursor = jay_after_inst(I);
         break;
      }

      jay_foreach_ra_src(I, s) {
         jay_set_reg(&I->src[s],
                     r_reg(ra->global_reg_for_index[jay_index(I->src[s])]));
      }
   }

   const unsigned num_pcopies =
      util_dynarray_num_elements(&copies, struct jay_parallel_copy);

   jay_emit_parallel_copies(&ra->bld, copies.data, num_pcopies, temp_regs);
   util_dynarray_fini(&copies);
}

/*
 * Record all phi webs. First initialize the union-find data structure
 * with all SSA defs in their own singletons, then union together anything
 * related by a phi. The resulting union-find structure will be the webs.
 */
static void
construct_phi_webs(struct phi_web_node *web, jay_function *f)
{
   for (unsigned i = 0; i < f->ssa_alloc; ++i) {
      web[i] = (struct phi_web_node) { .parent = i, .reg = NO_REG };
   }

   jay_foreach_block(f, block) {
      jay_foreach_phi_src_in_block(block, phi) {
         phi_web_union(web, jay_index(phi->src[0]), jay_phi_src_index(phi));
      }
   }
}

static void
insert_parallel_copies_for_phis(jay_function *f)
{
   jay_reg *phi_dsts = calloc(f->ssa_alloc, sizeof(jay_reg));
   struct util_dynarray copies = UTIL_DYNARRAY_INIT;
   memset(phi_dsts, 0xFF, sizeof(jay_reg) * f->ssa_alloc);

   jay_foreach_block(f, block) {
      jay_foreach_phi_dst_in_block(block, I) {
         phi_dsts[jay_index(I->dst)] = make_reg(I->dst.file, I->dst.reg);
      }
   }

   jay_foreach_block(f, block) {
      jay_builder b = jay_init_builder(f, jay_before_jump(block));

      /* Copy phi source to phi destination along the edge. */
      jay_foreach_phi_src_in_block(block, phi) {
         jay_reg src = make_reg(phi->src[0].file, phi->src[0].reg);
         add_copy(&copies, phi_dsts[jay_phi_src_index(phi)], src);
         jay_remove_instruction(phi);
      }

      const unsigned nr =
         util_dynarray_num_elements(&copies, struct jay_parallel_copy);

      jay_emit_parallel_copies(&b, copies.data, nr, block->temps_out);
      util_dynarray_clear(&copies);
   }

   util_dynarray_fini(&copies);
   free(phi_dsts);
}

static struct jay_register_block
block_gpr_to_grf(struct jay_partition *p, enum jay_file file, unsigned block)
{
   assert(file == GPR || file == UGPR);
   assert(((p->blocks[file][block].start * 16) % p->units_x16[file]) == 0);
   assert(((p->blocks[file][block].len * 16) % p->units_x16[file]) == 0);

   return (struct jay_register_block) {
      .start = (p->blocks[file][block].start * 16) / p->units_x16[file],
      .len = (p->blocks[file][block].len * 16) / p->units_x16[file],
   };
}

static void
print_partition(struct jay_partition *p)
{
   for (unsigned f = 0; f < JAY_NUM_GRF_FILES; ++f) {
      for (unsigned b = 0; b < JAY_PARTITION_BLOCKS; ++b) {
         struct jay_register_block B = block_gpr_to_grf(p, f, b);
         const char *file = f ? "UGPR" : "GPR";

         if (B.len > 1) {
            fprintf(stderr, "%s: %u-%u\n", file, B.start, B.start + B.len - 1);
         } else if (B.len == 1) {
            fprintf(stderr, "%s: %u\n", file, B.start);
         }
      }
   }

   fprintf(stderr, "\n");
}

/*
 * Verify that a register partition is a bijective mapping of the GRF file.
 */
static void
validate_partition(struct jay_partition *p,
                   unsigned stride4_header_size,
                   unsigned nonuniform_gprs)
{
   BITSET_DECLARE(regs, JAY_NUM_PHYS_GRF) = { 0 };

   for (unsigned f = 0; f < JAY_NUM_GRF_FILES; ++f) {
      for (unsigned b = 0; b < JAY_PARTITION_BLOCKS; ++b) {
         struct jay_register_block B = block_gpr_to_grf(p, f, b);
         if (B.len) {
            assert(B.start + B.len <= JAY_NUM_PHYS_GRF && "GRF file size");
            assert(!BITSET_TEST_COUNT(regs, B.start, B.len) && "uniqueness");

            BITSET_SET_COUNT(regs, B.start, B.len);
         }
      }
   }

   for (unsigned i = 0; i < JAY_NUM_PHYS_GRF; ++i) {
      assert(BITSET_TEST(regs, i) && "all GRFs mapped");
   }

   assert(p->large_ugpr_block.len && "partition must have a large UGPR block");
   assert(p->base2 >= p->base8 && p->base_eot >= p->base2 && "monotonic");
   assert(p->base8 >= stride4_header_size && "header is big enough");
   assert(p->base_eot + p->units_x16[GPR] <= nonuniform_gprs && "EOT fits");
   assert(util_is_aligned(p->base8, 8) && "so vectors don't cross");
   assert(util_is_aligned(p->base2, 8) && "so vectors don't cross");
   assert(util_is_aligned(p->base_eot, 8) && "so vectors don't cross");
}

static void
build_partition(jay_shader *shader, unsigned *blocks, unsigned n)
{
   unsigned base = 0;
   unsigned ugpr_base = 0;
   struct jay_partition *p = &shader->partition;

   *p = (struct jay_partition) {
      .units_x16[UGPR] = jay_ugpr_per_grf(shader) * 16,
      .units_x16[GPR] = 16 / jay_grf_per_gpr(shader),
   };

   for (unsigned i = 0; i < n; ++i) {
      enum jay_file file = (i & 1) ? GPR : UGPR;
      unsigned file_i = i >> 1;

      p->blocks[file][file_i].start = (base * p->units_x16[file]) / 16;
      p->blocks[file][file_i].len = (blocks[i] * p->units_x16[file]) / 16;

      if (file == UGPR && blocks[i] >= 8) {
         p->large_ugpr_block = (struct jay_register_block) {
            .start = (ugpr_base * p->units_x16[file]) / 16,
            .len = p->blocks[file][file_i].len,
         };
      }

      base += blocks[i];
      if (file == UGPR) {
         ugpr_base += blocks[i];
      }

      /* GPR partition blocks must be vector size aligned to avoid crossing */
      if (file == GPR && i != (n - 1)) {
         unsigned max_vec = 8;
         assert(util_is_aligned(blocks[i], max_vec * jay_grf_per_gpr(shader)));
      }
   }
}

/*
 * Partition the register file for the entire shader. All functions must
 * share the same partition for correctness with non-uniform function calls.
 * For unlinked library functions, we must use the ABI partition (TODO).
 */
void
jay_partition_grf(jay_shader *shader)
{
   /* Calculate the maximum register demand across all functions in the shader.
    * We will use this to choose a good partition.
    */
   struct jay_partition *p = &shader->partition;
   unsigned demand[JAY_NUM_GRF_FILES] = { 0 };

   jay_foreach_function(shader, f) {
      jay_compute_liveness(f);
      jay_calculate_register_demands(f);

      demand[GPR] = MAX2(demand[GPR], f->demand[GPR]);
      demand[UGPR] = MAX2(demand[UGPR], f->demand[UGPR]);
   }

   /* We must have enough register file space for the register payload, plus the
    * reserved UGPRs in the case we spill. That UGPR interferes with everything
    * we preload so it needs to be reserved specially here for the worst case.
    */
   jay_foreach_preload(jay_shader_get_entrypoint(shader), I) {
      unsigned end = jay_preload_reg(I) + jay_num_values(I->dst);
      unsigned extra = I->dst.file == UGPR ? shader->dispatch_width + 1 : 0;
      assert(I->dst.file < JAY_NUM_GRF_FILES);
      demand[I->dst.file] = MAX2(demand[I->dst.file], end + extra);
   }

   /* Determine a good GPR/UGPR split informed by the demand calculation */
   unsigned ugpr_per_grf = jay_ugpr_per_grf(shader);
   unsigned uniform_grfs = DIV_ROUND_UP(demand[UGPR], ugpr_per_grf);

   /* We must have enough for SIMD1 images (TODO: Check if this actually
    * applies. Or if we could eliminate this with smarter partitioning even.)
    */
   unsigned min_ugprs = 16;
   min_ugprs = MAX2(min_ugprs, 256);

   unsigned grf_block_alignment = 8 * jay_grf_per_gpr(shader); /* max_vec */

   /* TODO: We could partition more cleverly */
   uniform_grfs = CLAMP(align(uniform_grfs, grf_block_alignment),
                        DIV_ROUND_UP(min_ugprs, ugpr_per_grf),
                        128 - (32 * jay_grf_per_gpr(shader)));
   unsigned nonuniform_grfs = JAY_NUM_PHYS_GRF - uniform_grfs;

   /* Check the split */
   assert((uniform_grfs * ugpr_per_grf) >= min_ugprs);
   assert(nonuniform_grfs >= 32 * jay_grf_per_gpr(shader));
   assert((uniform_grfs + nonuniform_grfs) == JAY_NUM_PHYS_GRF);

   /* Partition GRFs between GPR & UGPR */
   unsigned dispatch_grf = 0;
   unsigned stride4_header_size = 0;

   if (shader->stage == MESA_SHADER_VERTEX) {
      unsigned attrib_grfs = shader->prog_data->vue.urb_read_length * 8;
      unsigned blocks[] = {
         1,                                         /* UGPR: g0 */
         8,                                         /* GPR: URB output handle */
         shader->push_grfs,                         /* UGPR: Push constants */
         attrib_grfs,                               /* GPR: Vertex inputs */
         uniform_grfs - (blocks[0] + blocks[2]),    /* UGPR: * */
         nonuniform_grfs - (blocks[1] + blocks[3]), /* GPR: * and EOT */
      };

      build_partition(shader, blocks, ARRAY_SIZE(blocks));
      dispatch_grf = blocks[0] + blocks[1];
      stride4_header_size = blocks[1] + blocks[3];
   } else if (shader->stage == MESA_SHADER_FRAGMENT) {
      unsigned len0 = jay_grf_per_gpr(shader);
      unsigned blocks[] = {
         len0,                /* UGPR: g0 (and maybe g1) */
         len0 * 8,            /* GPR: Barycentrics */
         uniform_grfs - len0, /* UGPR: Dispatch (eg push constants) & general */
         nonuniform_grfs - (len0 * 8), /* GPR: General & end-of-thread */
      };
      build_partition(shader, blocks, ARRAY_SIZE(blocks));
      dispatch_grf = blocks[0] + blocks[1];
      stride4_header_size = blocks[1];
   } else {
      unsigned blocks[] = { uniform_grfs - 4, nonuniform_grfs, 4 };
      build_partition(shader, blocks, ARRAY_SIZE(blocks));
   }

   /* TODO: Make the stride partition smarter */
   unsigned nonuniform_gprs = nonuniform_grfs / jay_grf_per_gpr(shader);
   unsigned eot_gprs = 16 / jay_grf_per_gpr(shader);
   p->base8 = ROUND_DOWN_TO(nonuniform_gprs - (16 + eot_gprs), 8) + 0;
   p->base2 = 8 + p->base8;
   p->base_eot = 8 + p->base2;

   // print_partition(p);
   validate_partition(p, stride4_header_size, nonuniform_gprs);

   if (shader->stage == MESA_SHADER_FRAGMENT && shader->dispatch_width == 32) {
      shader->prog_data->fs.dispatch_grf_start_reg_32 = dispatch_grf;
   } else if (shader->stage == MESA_SHADER_FRAGMENT &&
              shader->dispatch_width == 16) {
      shader->prog_data->fs.dispatch_grf_start_reg_16 = dispatch_grf;
   } else {
      shader->prog_data->base.dispatch_grf_start_reg = dispatch_grf;
   }

   /* By construction of our partition, the entire GRF is used. */
   shader->prog_data->base.grf_used = JAY_NUM_PHYS_GRF;

   /* Set the targets for the virtual register file accordingly */
   for (unsigned f = 0; f < JAY_NUM_GRF_FILES; ++f) {
      for (unsigned b = 0; b < JAY_PARTITION_BLOCKS; ++b) {
         shader->num_regs[f] += p->blocks[f][b].len;
      }
   }

   /* TODO: These are arbitrary. Need to rework somehow, we have options. */
   shader->num_regs[MEM] = 512;
   shader->num_regs[UMEM] = 2048;
}

static void
spill_file(jay_function *f, enum jay_file file, bool *spilled)
{
   unsigned limit = f->shader->num_regs[file];

   /* If testing spilling, set limit tightly. */
   if ((jay_debug & JAY_DBG_SPILL) &&
       file == GPR &&
       f->shader->stage != MESA_SHADER_VERTEX) {
      limit = 13;
   }

   /* Ensures we don't XOR swap, XXX: TODO: FIXME */
   limit--;

   if (f->demand[file] > limit) {
      /* In the worst case, we
       * require 2 temporary registers to lower a memory-memory swap produced by
       * parallel copy lowering, so adjust the limit to be num_regs - 2.
       */
      limit--;

      /* If we spill, we need to reserve UGPRs for spilling */
      if (!(*spilled)) {
         unsigned reservation = f->shader->dispatch_width + 1;
         f->shader->num_regs[UGPR] -= reservation;
         f->shader->partition.large_ugpr_block.len -= reservation;
      }

      jay_spill(f, file, limit);
      jay_validate(f->shader, "spilling");
      jay_compute_liveness(f);
      jay_calculate_register_demands(f);

      if (f->demand[file] > limit) {
         fprintf(stderr, "limit %u but demand %u\n", limit, f->demand[file]);
         UNREACHABLE("spiller bug");
      }

      *spilled = true;
   }
}

static void
jay_register_allocate_function(jay_function *f)
{
   jay_shader *shader = f->shader;
   jay_ra_state ra = { .bld.shader = shader, .bld.func = f };

   /* Spill as needed to fit within the limits. We spill GPR before UGPR since
    * spilling GPRs requires reserving a UGPR.
    */
   bool spilled = false;
   spill_file(f, GPR, &spilled);
   spill_file(f, UGPR, &spilled);

   typed_memcpy(ra.num_regs, shader->num_regs, JAY_NUM_RA_FILES);

   /* The end of the register file is allowed for end-of-thread messages.
    * Calculate the offset in GPRs. Compute shaders have this as UGPRs while
    * fragment shaders have this as GPRs.
    */
   if (mesa_shader_stage_is_compute(shader->stage)) {
      ra.eot_offs = ROUND_DOWN_TO(ra.num_regs[UGPR], jay_ugpr_per_grf(shader)) -
                    jay_ugpr_per_grf(shader);
   } else {
      ra.eot_offs = ra.num_regs[GPR] - (16 / jay_grf_per_gpr(shader));
   }

   linear_ctx *lin_ctx = linear_context(shader);

   ra.reg_for_index = linear_alloc_array(lin_ctx, jay_reg, f->ssa_alloc);
   ra.global_reg_for_index = linear_alloc_array(lin_ctx, jay_reg, f->ssa_alloc);
   ra.affinities = linear_zalloc_array(lin_ctx, struct affinity, f->ssa_alloc);

   memset(ra.reg_for_index, 0xFF, sizeof(jay_reg) * f->ssa_alloc);
   memset(ra.global_reg_for_index, 0xFF, sizeof(jay_reg) * f->ssa_alloc);

   jay_foreach_ra_file(file) {
      const unsigned num_regs = ra.num_regs[file];
      ra.index_for_reg[file] = linear_zalloc_array(lin_ctx, uint32_t, num_regs);
      ra.available_regs[file] = BITSET_LINEAR_ZALLOC(lin_ctx, num_regs);
      ra.pinned[file] = BITSET_LINEAR_ZALLOC(lin_ctx, num_regs);
   }

   ra.phi_web = linear_zalloc_array(lin_ctx, struct phi_web_node, f->ssa_alloc);

   /* Construct the phi equivalence classes using the union-find data
    * structure. This associates all SSA values related to the same phi,
    * and selects one of them as a canonical/representative value.
    */
   construct_phi_webs(ra.phi_web, f);

   jay_foreach_inst_in_func(f, block, I) {
      jay_foreach_src_index(I, s, c, index) {
         /* We check repr==0 to try to coalesce with the first vector use, as
          * the closest to the definition. This heuristic reduces shuffling.
          */
         if (jay_num_values(I->src[s]) > 1 && !ra.affinities[index].repr) {
            uint32_t repr = UINT_MAX, repr_c = 0;

            /* Pick the representative with the smallest index, as it most
             * likely dominates the other components.
             */
            jay_foreach_comp(I->src[s], j) {
               if (jay_channel(I->src[s], j) < repr) {
                  repr = jay_channel(I->src[s], j);
                  repr_c = j;
               }
            }

            ra.affinities[index].repr = repr;
            ra.affinities[index].offset = repr == index ? c : c - repr_c;
         }

         if (I->op == JAY_OPCODE_SEND && jay_send_eot(I)) {
            ra.affinities[index].eot = true;
         }

         if (jay_src_alignment(shader, I, s) >= ra.affinities[index].align) {
            ra.affinities[index].align = jay_src_alignment(shader, I, s);
            ra.affinities[index].align_offs = c;
         }

         ra.phi_web[phi_web_find(ra.phi_web, index)].affinity =
            ra.affinities[index];
      }
   }

   jay_foreach_block(f, block) {
      local_ra(&ra, block);
   }

   linear_free_context(lin_ctx);

   /* Validate the registers we picked before going out of SSA */
   jay_validate_ra(f);

   insert_parallel_copies_for_phis(f);

   /* Lower spills using the UGPRs we stole above. We need to update num_regs
    * for correct scoreboarding calculations.
    */
   if (spilled) {
      jay_lower_spill(f);
      f->shader->num_regs[UGPR] += f->shader->dispatch_width + 1;
   }
}

void
jay_register_allocate(jay_shader *s)
{
   jay_foreach_function(s, f) {
      jay_register_allocate_function(f);
   }

   s->post_ra = true;
}
