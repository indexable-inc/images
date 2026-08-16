/*
 * Copyright © 2018 Intel Corporation
 * SPDX-License-Identifier: MIT
 */

#include "brw_asm.h"
#include "brw_asm_internal.h"
#include "brw_disasm_info.h"
#include "util/hash_table.h"
#include "util/u_dynarray.h"

typedef struct {
   char *name;
   int offset; /* -1 for unset */
   struct util_dynarray jip_uses;
   struct util_dynarray uip_uses;
} brw_asm_label;

static brw_asm_label *
brw_asm_label_lookup(struct brw_asm_parser *parser, const char *name)
{
   uint32_t h = _mesa_hash_string(name);
   struct hash_entry *entry =
      _mesa_hash_table_search_pre_hashed(parser->labels, h, name);
   if (!entry) {
      void *mem_ctx = parser->labels;
      brw_asm_label *label = rzalloc(mem_ctx, brw_asm_label);
      label->name = ralloc_strdup(mem_ctx, name);
      label->offset = -1;
      util_dynarray_init(&label->jip_uses, mem_ctx);
      util_dynarray_init(&label->uip_uses, mem_ctx);
      entry = _mesa_hash_table_insert_pre_hashed(parser->labels,
                                                 h, name, label);
   }
   assert(entry);
   return entry->data;
}

void
brw_asm_label_set(struct brw_asm_parser *parser, const char *name)
{
   brw_asm_label *label = brw_asm_label_lookup(parser, name);
   label->offset = parser->p->next_insn_offset;
}

void
brw_asm_label_use_jip(struct brw_asm_parser *parser, const char *name)
{
   struct brw_codegen *p = parser->p;
   brw_asm_label *label = brw_asm_label_lookup(parser, name);
   int offset = p->next_insn_offset - sizeof(brw_eu_inst);
   util_dynarray_append(&label->jip_uses, offset);
   /* Will be patched later. */
   brw_eu_inst_set_jip(p->devinfo, brw_last_inst, 0);
}

void
brw_asm_label_use_uip(struct brw_asm_parser *parser, const char *name)
{
   struct brw_codegen *p = parser->p;
   brw_asm_label *label = brw_asm_label_lookup(parser, name);
   int offset = p->next_insn_offset - sizeof(brw_eu_inst);
   util_dynarray_append(&label->uip_uses, offset);
   /* Will be patched later. */
   brw_eu_inst_set_uip(p->devinfo, brw_last_inst, 0);
}

static bool
brw_postprocess_labels(struct brw_asm_parser *parser)
{
   unsigned unknown = 0;
   struct brw_codegen *p = parser->p;
   void *store = p->store;

   hash_table_foreach(parser->labels, entry) {
      brw_asm_label *label = entry->data;

      if (label->offset == -1) {
         fprintf(stderr, "Unknown label '%s'\n", label->name);
         unknown++;
         continue;
      }

      util_dynarray_foreach(&label->jip_uses, int, use_offset) {
         brw_eu_inst *inst = store + *use_offset;
         brw_eu_inst_set_jip(parser->devinfo, inst, label->offset - *use_offset);
      }

      util_dynarray_foreach(&label->uip_uses, int, use_offset) {
         brw_eu_inst *inst = store + *use_offset;
         brw_eu_inst_set_uip(parser->devinfo, inst, label->offset - *use_offset);
      }
   }

   return unknown == 0;
}

/* TODO: Would be nice to make this operate on string instead on a FILE. */

brw_assemble_result
brw_assemble(void *mem_ctx, const struct intel_device_info *devinfo,
             FILE *f, const char *filename, brw_assemble_flags flags)
{
   brw_assemble_result result = {0};

   struct brw_isa_info isa;
   brw_init_isa_info(&isa, devinfo);

   /* This is allocated separatedly from the parser since will outlive
    * the parser state.
    */
   struct brw_codegen *p = rzalloc(mem_ctx, struct brw_codegen);
   brw_init_codegen(&isa, p, p);

   brw_asm_parser *parser = rzalloc(mem_ctx, brw_asm_parser);
   parser->devinfo = devinfo;
   parser->labels = _mesa_string_hash_table_create(parser);
   parser->p = p;
   parser->input_filename = filename;
   parser->compaction_warning_given = false;

   parser->scanner = NULL;
   brw_asm_lex_init_extra(parser, &parser->scanner);
   brw_asm_restart(f, parser->scanner);

   int err = yyparse(parser);
   brw_asm_lex_destroy(parser->scanner);
   if (err || parser->errors)
      goto end;

   if (!brw_postprocess_labels(parser))
      goto end;

   struct disasm_info *disasm_info = disasm_initialize(p->isa, NULL);
   if (!disasm_info) {
      ralloc_free(disasm_info);
      fprintf(stderr, "Unable to initialize disasm_info struct instance\n");
      goto end;
   }

   /* Add "inst groups" so validation errors can be recorded. */
   for (int i = 0; i <= p->next_insn_offset; i += 16)
      disasm_new_inst_group(disasm_info, i);

   if (!brw_validate_instructions(p->isa, p->store, 0,
                                  p->next_insn_offset, disasm_info)) {
      dump_assembly(p->store, 0, p->next_insn_offset, disasm_info, NULL, stderr);
      ralloc_free(disasm_info);
      fprintf(stderr, "Invalid instructions.\n");
      goto end;
   }

   if ((flags & BRW_ASSEMBLE_COMPACT) != 0)
      brw_compact_instructions(p, 0, disasm_info);

   result.bin = p->store;
   result.bin_size = p->next_insn_offset;

   if ((flags & BRW_ASSEMBLE_DUMP) != 0)
      dump_assembly(p->store, 0, p->next_insn_offset, disasm_info, NULL, stderr);

   ralloc_free(disasm_info);

end:
   ralloc_free(parser);

   return result;
}

