//! Stage 0 (`skeleton` subcommand, #3413): reduce a kernel source tree to the
//! inputs the *plan* build actually depends on, so a function-body edit never
//! reruns the plan.
//!
//! Every compiled source (`**/*.c`, `**/*.S`) is stripped to its preprocessor
//! directives; everything else (headers, Makefiles, Kconfig, `scripts/**`,
//! `tools/**`, linker scripts, `.dts`, ...) is copied byte-verbatim. The
//! output is a plain copy, never symlinks, so its content (and therefore its
//! content-addressed store path) is independent of the input's store path:
//! two source trees that differ only in function bodies reduce to identical
//! skeletons, the plan derivation's resolved inputs stay unchanged, and the
//! already-realised plan is reused. Directive lines (`^\s*#`, with their
//! backslash continuations) are preserved because they are exactly what
//! determines the recorded dep set (`-Wp,-MMD`) and the saved command flags;
//! bodies only influence object bytes, which the plan's stub objects (see
//! `lib/kernel/kbuild-plan-cc.sh`) discard anyway.
//!
//! Reduced files start with [`MARKER`] so the plan's compiler shim can
//! recognize them and substitute stub objects without any allowlist plumbing:
//! the reducer's keep decision *is* the shim's stub decision.

use std::path::{Path, PathBuf};

use color_eyre::eyre::{WrapErr as _, bail};

/// First line of every reduced source file. The plan's `gcc` shim stubs a
/// compile exactly when its source operand starts with this line, so the
/// reduction decision made here is the single source of truth.
pub const MARKER: &str =
    "/* nix-kbuild-unit skeleton: reduced to preprocessor directives (#3413) */\n";

/// Sources kept whole even though they match `**/*.c` / `**/*.S`: their
/// compiled output feeds the generated snapshot, the vdso/realmode blobs, or
/// a host tool, so a reduced source would corrupt what units later consume
/// (or fail the plan build loudly). Globs: `*` matches within one path
/// segment, `**` matches any number of segments.
///
/// Deliberately NOT kept: kernel-side TUs that merely define symbols the
/// linker script references (`jiffies_64`, mitigation thunks, ...). Keeping
/// them real would drag their relocations against stubbed code into the
/// final plan-time link; instead the plan's `ld` shim `--defsym`s that
/// symbol set (see lib/kernel/kbuild-plan-ld.sh).
pub const DEFAULT_KEEP: &[&str] = &[
    // Their `-S` listings become `include/generated/` headers every unit
    // reads from the snapshot.
    "arch/*/kernel/asm-offsets*.c",
    "kernel/bounds.c",
    // Sources feeding the vdso `.so` images, which are built at plan time
    // and re-enter the unit graph via the generated `vdso-image-*.c`. The
    // kernel-side files in the same dir (vma.c, extable.c, vdso32-setup.c)
    // stay reduced: their objects link into vmlinux, and real code there
    // would reference stubbed symbols. x86-specific paths are fine: harvest
    // rejects non-x86 plans (detect_srcarch).
    "arch/x86/entry/vdso/vclock_gettime.c",
    "arch/x86/entry/vdso/vgetcpu.c",
    "arch/x86/entry/vdso/vdso-note.S",
    "arch/x86/entry/vdso/vsgx.S",
    "arch/x86/entry/vdso/vdso2c.c",
    "arch/x86/entry/vdso/vdso32/**",
    "lib/vdso/**",
    // The realmode blob: rm/ links realmode.elf from real objects with its
    // own linker script, and rmpiggy.S incbins the resulting .bin (its
    // .relocs dump rides in the snapshot). arch/x86/realmode/init.c stays
    // reduced for the same reason as vma.c above.
    "arch/x86/realmode/rm/**",
    "arch/x86/realmode/rmpiggy.S",
    "arch/*/boot/**",
    // Assembly helpers that are only ever #included by the kept realmode
    // and boot code (never compiled standalone, so they add nothing to the
    // stub vmlinux link): reducing them breaks the realmode.elf link with
    // undefined verify_cpu. boot/compressed pulls more .c includes
    // (ident_map.c, tdx-shared.c, ...), but nothing under arch/*/boot/
    // builds during the plan's `make vmlinux modules`; extend this when a
    // bzImage target enters the plan.
    "arch/x86/kernel/verify_cpu.S",
    "arch/x86/kernel/sev_verify_cbit.S",
    // Host-tool sources compiled outside scripts/ and tools/: HOSTCC passes
    // through the compiler shim (no -D__KERNEL__), so a reduced host source
    // would link a main-less tool and fail the plan loudly (that is how a
    // missing entry here surfaces). Enumerated from the 6.12 tree's
    // `hostprogs` Makefile declarations.
    "arch/*/tools/**",
    "certs/extract-cert.c",
    "drivers/accessibility/speakup/genmap.c",
    "drivers/accessibility/speakup/makemapdata.c",
    "drivers/gpu/drm/radeon/mkregtable.c",
    "drivers/gpu/drm/xe/xe_gen_wa_oob.c",
    "drivers/tty/vt/conmakehash.c",
    "drivers/video/logo/pnmtologo.c",
    "drivers/zorro/gen-devlist.c",
    "fs/unicode/mkutf8data.c",
    "lib/gen_crc32table.c",
    "lib/gen_crc64table.c",
    "lib/raid6/mktables.c",
    "usr/gen_init_cpio.c",
];

/// Reduce `src` into `out` (created if absent), keeping `extra_keep` globs
/// verbatim on top of [`DEFAULT_KEEP`].
pub fn skeleton(src: &Path, out: &Path, extra_keep: &[String]) -> color_eyre::Result<()> {
    std::fs::create_dir_all(out).wrap_err_with(|| format!("creating {}", out.display()))?;
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let dir = src.join(&rel_dir);
        let entries =
            std::fs::read_dir(&dir).wrap_err_with(|| format!("listing {}", dir.display()))?;
        for entry in entries {
            let entry = entry.wrap_err_with(|| format!("listing {}", dir.display()))?;
            let file_type = entry.file_type()?;
            let rel = rel_dir.join(entry.file_name());
            let dst = out.join(&rel);
            if file_type.is_dir() {
                std::fs::create_dir(&dst)
                    .wrap_err_with(|| format!("creating {}", dst.display()))?;
                stack.push(rel);
                continue;
            }
            let Some(rel_str) = rel.to_str() else {
                bail!("non-UTF-8 path in source tree: {}", rel.display());
            };
            if file_type.is_symlink() {
                let target = std::fs::read_link(entry.path())
                    .wrap_err_with(|| format!("readlink {rel_str}"))?;
                std::os::unix::fs::symlink(&target, &dst)
                    .wrap_err_with(|| format!("recreating symlink {rel_str}"))?;
                continue;
            }
            match reduction_lang(rel_str, extra_keep) {
                Some(lang) => {
                    let bytes = std::fs::read(entry.path())
                        .wrap_err_with(|| format!("reading {rel_str}"))?;
                    std::fs::write(&dst, reduce(&bytes, lang))
                        .wrap_err_with(|| format!("writing {rel_str}"))?;
                }
                None => {
                    // fs::copy preserves permission bits, so kept scripts
                    // stay executable.
                    std::fs::copy(entry.path(), &dst)
                        .wrap_err_with(|| format!("copying {rel_str}"))?;
                }
            }
        }
    }
    Ok(())
}

/// How a reduced file is tokenized: `'` starts a character literal in C, but
/// gas allows unterminated `'a` character operands, so treating `'` as a
/// delimiter in assembly could swallow the rest of the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Lang {
    C,
    Asm,
}

/// Whether `rel` gets reduced, and as which language. Only `**/*.c` and
/// `**/*.S` reduce; `*.lds.S` linker scripts, host sources under `scripts/`
/// and `tools/`, and allowlist matches stay verbatim.
fn reduction_lang(rel: &str, extra_keep: &[String]) -> Option<Lang> {
    if rel.ends_with(".lds.S") || rel.starts_with("scripts/") || rel.starts_with("tools/") {
        return None;
    }
    let lang = if rel.ends_with(".c") {
        Lang::C
    } else if rel.ends_with(".S") {
        Lang::Asm
    } else {
        return None;
    };
    if DEFAULT_KEEP.iter().any(|glob| glob_match(glob, rel))
        || extra_keep.iter().any(|glob| glob_match(glob, rel))
    {
        return None;
    }
    Some(lang)
}

/// Strip comments (string-literal-aware), then keep only preprocessor
/// directive lines (`^\s*#`) with their backslash-continuation lines. The
/// output starts with [`MARKER`].
fn reduce(bytes: &[u8], lang: Lang) -> Vec<u8> {
    let stripped = strip_comments(bytes, lang);
    let lines: Vec<&[u8]> = stripped.split(|&b| b == b'\n').collect();
    let mut out = Vec::with_capacity(MARKER.len() + stripped.len() / 4);
    out.extend_from_slice(MARKER.as_bytes());
    let mut i = 0;
    while i < lines.len() {
        // One logical line: physical lines spliced by trailing backslashes.
        let mut last = i;
        while last + 1 < lines.len() && lines[last].ends_with(b"\\") {
            last += 1;
        }
        let is_directive =
            lines[i].iter().copied().find(|&b| b != b' ' && b != b'\t') == Some(b'#');
        if is_directive {
            for line in &lines[i..=last] {
                out.extend_from_slice(line);
                out.push(b'\n');
            }
        }
        i = last + 1;
    }
    out
}

/// Replace comments with whitespace, tracking string and (for C) character
/// literals so a `//` or `/*` inside a literal is not a comment, and a `#`
/// inside a comment never reaches the directive scan.
///
/// Block comments collapse to a single space with interior newlines dropped:
/// a comment inside a directive is whitespace, not a line break, so
/// `#define X /* multi\nline */ 1` must stay one logical line. (Valid code
/// cannot hide a directive boundary inside a block comment: a `#` is only a
/// directive when it follows a real newline, which a comment never contains
/// after phase-3 replacement.) Line comments swallow their backslash
/// continuations; their terminating newline stays.
fn strip_comments(bytes: &[u8], lang: Lang) -> Vec<u8> {
    #[derive(PartialEq)]
    enum State {
        Normal,
        DoubleQuote,
        SingleQuote,
    }
    let mut out = Vec::with_capacity(bytes.len());
    let mut state = State::Normal;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        match state {
            State::Normal => {
                if b == b'/' && bytes.get(i + 1) == Some(&b'/') {
                    i += 2;
                    while i < bytes.len() && bytes[i] != b'\n' {
                        // A backslash-newline splices the next physical line
                        // into the comment.
                        if bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'\n') {
                            i += 2;
                        } else {
                            i += 1;
                        }
                    }
                    continue;
                }
                if b == b'/' && bytes.get(i + 1) == Some(&b'*') {
                    out.push(b' ');
                    i += 2;
                    while i < bytes.len() {
                        if bytes[i] == b'*' && bytes.get(i + 1) == Some(&b'/') {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    continue;
                }
                if b == b'"' {
                    state = State::DoubleQuote;
                } else if b == b'\'' && lang == Lang::C {
                    state = State::SingleQuote;
                }
                out.push(b);
                i += 1;
            }
            State::DoubleQuote | State::SingleQuote => {
                if b == b'\\' && i + 1 < bytes.len() {
                    out.push(b);
                    out.push(bytes[i + 1]);
                    i += 2;
                    continue;
                }
                let closing = match state {
                    State::DoubleQuote => b'"',
                    _ => b'\'',
                };
                if b == closing {
                    state = State::Normal;
                }
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

/// Match `path` against a `/`-separated glob: `**` spans any number of
/// segments, `*` matches within one segment. No other metacharacters.
fn glob_match(pattern: &str, path: &str) -> bool {
    let pattern: Vec<&str> = pattern.split('/').collect();
    let path: Vec<&str> = path.split('/').collect();
    match_segments(&pattern, &path)
}

fn match_segments(pattern: &[&str], path: &[&str]) -> bool {
    let Some((first, rest)) = pattern.split_first() else {
        return path.is_empty();
    };
    if *first == "**" {
        return (0..=path.len()).any(|skip| match_segments(rest, &path[skip..]));
    }
    let Some((seg, path_rest)) = path.split_first() else {
        return false;
    };
    segment_match(first.as_bytes(), seg.as_bytes()) && match_segments(rest, path_rest)
}

/// Classic backtracking wildcard match; `*` matches any run of bytes.
fn segment_match(pattern: &[u8], segment: &[u8]) -> bool {
    let (mut pi, mut si) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while si < segment.len() {
        if pattern.get(pi) == Some(&b'*') {
            star = Some((pi, si));
            pi += 1;
        } else if pattern.get(pi) == Some(&segment[si]) {
            pi += 1;
            si += 1;
        } else if let Some((star_pi, star_si)) = star {
            pi = star_pi + 1;
            si = star_si + 1;
            star = Some((star_pi, star_si + 1));
        } else {
            return false;
        }
    }
    while pattern.get(pi) == Some(&b'*') {
        pi += 1;
    }
    pi == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reduced(text: &str, lang: Lang) -> String {
        String::from_utf8(reduce(text.as_bytes(), lang)).expect("reduced output is UTF-8")
    }

    fn body(text: &str, lang: Lang) -> String {
        let out = reduced(text, lang);
        out.strip_prefix(MARKER).expect("marker prefix").to_owned()
    }

    #[test]
    fn keeps_directives_drops_bodies() {
        let src = "\
#include <linux/kernel.h>
#include \"internal.h\"

static int add(int a, int b)
{
\treturn a + b;
}

#ifdef CONFIG_SMP
int smp_thing;
#else
int up_thing;
#endif
";
        assert_eq!(
            body(src, Lang::C),
            "#include <linux/kernel.h>\n#include \"internal.h\"\n#ifdef CONFIG_SMP\n#else\n#endif\n"
        );
    }

    #[test]
    fn keeps_backslash_continuations() {
        let src = "\
#define LONG_MACRO(x) \\
\tdo { (x)++; } while (0)
int not_a_directive = 1;
";
        assert_eq!(
            body(src, Lang::C),
            "#define LONG_MACRO(x) \\\n\tdo { (x)++; } while (0)\n"
        );
        // A continuation chain keeps every physical line, even ones that do
        // not themselves look like directives.
        let chain = "#define A \\\n b \\\n c\n";
        assert_eq!(body(chain, Lang::C), chain);
    }

    #[test]
    fn continuation_from_code_line_swallows_directive_lookalike() {
        // The splice makes the `#` mid-line, so the original never treated it
        // as a directive either.
        let src = "int a = b \\\n#define X\n";
        assert_eq!(body(src, Lang::C), "");
    }

    #[test]
    fn comment_embedded_hash_is_not_a_directive() {
        let src = "\
/*
 * # this hash starts a line inside a comment
 *#include <not/real.h>
 */
// #define ALSO_NOT_REAL
#define REAL 1
";
        assert_eq!(body(src, Lang::C), "#define REAL 1\n");
    }

    #[test]
    fn strings_hide_comment_openers() {
        let src = "\
#define URL \"http://example.com\"
static const char *s = \"/* not a comment\";
#define AFTER 2
";
        assert_eq!(
            body(src, Lang::C),
            "#define URL \"http://example.com\"\n#define AFTER 2\n"
        );
    }

    #[test]
    fn char_literal_hides_quote_in_c_but_not_asm() {
        // '"' must not open a string in C...
        let src = "#define Q '\"' // quote\n#define AFTER 3\n";
        assert_eq!(body(src, Lang::C), "#define Q '\"' \n#define AFTER 3\n");
        // ...while gas-style unterminated 'a operands must not swallow the
        // rest of an assembly file.
        let asm = "mov $'a, %eax\n#define AFTER 4\n";
        assert_eq!(body(asm, Lang::Asm), "#define AFTER 4\n");
    }

    #[test]
    fn trailing_line_comment_on_directive_is_stripped() {
        let src = "#include <a.h> // why\n#include <b.h> /* also why */\n";
        assert_eq!(body(src, Lang::C), "#include <a.h> \n#include <b.h>  \n");
    }

    #[test]
    fn line_comment_continuation_stays_comment() {
        // The backslash splices the next line into the comment, not into the
        // macro.
        let src = "#define X 1 // comment \\\n   still comment\n#define Y 2\n";
        assert_eq!(body(src, Lang::C), "#define X 1 \n#define Y 2\n");
    }

    #[test]
    fn block_comment_inside_directive_stays_one_logical_line() {
        // Phase-3 comment replacement makes this one directive defining X as
        // 1; dropping the second physical line would redefine X as empty.
        let src = "#define X /* multi\nline */ 1\nint code;\n";
        assert_eq!(body(src, Lang::C), "#define X   1\n");
    }

    #[test]
    fn marker_prefixes_reduced_output() {
        let out = reduced("int x;\n", Lang::C);
        assert!(out.starts_with(MARKER));
        assert!(MARKER.starts_with("/* nix-kbuild-unit skeleton:"));
    }

    #[test]
    fn reduction_scope() {
        let no_extra: &[String] = &[];
        // Kernel sources reduce.
        assert_eq!(reduction_lang("kernel/fork.c", no_extra), Some(Lang::C));
        assert_eq!(
            reduction_lang("arch/x86/kernel/head_64.S", no_extra),
            Some(Lang::Asm)
        );
        // Linker scripts, host tools, headers, build files stay verbatim.
        assert_eq!(
            reduction_lang("arch/x86/kernel/vmlinux.lds.S", no_extra),
            None
        );
        assert_eq!(reduction_lang("scripts/mod/modpost.c", no_extra), None);
        assert_eq!(reduction_lang("tools/objtool/check.c", no_extra), None);
        assert_eq!(reduction_lang("include/linux/kernel.h", no_extra), None);
        assert_eq!(reduction_lang("kernel/Makefile", no_extra), None);
        // DEFAULT_KEEP allowlist hits stay verbatim.
        assert_eq!(
            reduction_lang("arch/x86/kernel/asm-offsets.c", no_extra),
            None
        );
        assert_eq!(
            reduction_lang("arch/x86/kernel/asm-offsets_64.c", no_extra),
            None
        );
        assert_eq!(reduction_lang("kernel/bounds.c", no_extra), None);
        assert_eq!(
            reduction_lang("arch/x86/entry/vdso/vgetcpu.c", no_extra),
            None
        );
        assert_eq!(reduction_lang("lib/vdso/gettimeofday.c", no_extra), None);
        assert_eq!(
            reduction_lang("arch/x86/realmode/rm/wakemain.c", no_extra),
            None
        );
        assert_eq!(
            reduction_lang("arch/x86/realmode/rmpiggy.S", no_extra),
            None
        );
        // Host-tool sources outside scripts// tools/ stay whole; their
        // kernel-side dir siblings still reduce.
        assert_eq!(reduction_lang("arch/x86/tools/relocs_64.c", no_extra), None);
        assert_eq!(reduction_lang("lib/gen_crc32table.c", no_extra), None);
        assert_eq!(reduction_lang("usr/gen_init_cpio.c", no_extra), None);
        assert_eq!(
            reduction_lang("drivers/tty/vt/conmakehash.c", no_extra),
            None
        );
        assert_eq!(
            reduction_lang("drivers/tty/vt/vt.c", no_extra),
            Some(Lang::C)
        );
        // Kernel-side files in kept dirs reduce: their objects link into
        // vmlinux and must stay stubbed.
        assert_eq!(
            reduction_lang("arch/x86/entry/vdso/vma.c", no_extra),
            Some(Lang::C)
        );
        assert_eq!(
            reduction_lang("arch/x86/realmode/init.c", no_extra),
            Some(Lang::C)
        );
        assert_eq!(
            reduction_lang("kernel/time/timer.c", no_extra),
            Some(Lang::C)
        );
        assert_eq!(
            reduction_lang("arch/x86/boot/compressed/misc.c", no_extra),
            None
        );
        // --keep extends the allowlist.
        let extra = vec!["drivers/net/**".to_owned()];
        assert_eq!(reduction_lang("drivers/net/dummy.c", &extra), None);
        assert_eq!(reduction_lang("drivers/gpu/nope.c", &extra), Some(Lang::C));
    }

    #[test]
    fn glob_semantics() {
        assert!(glob_match(
            "arch/*/kernel/asm-offsets*.c",
            "arch/x86/kernel/asm-offsets.c"
        ));
        assert!(glob_match(
            "arch/*/kernel/asm-offsets*.c",
            "arch/arm64/kernel/asm-offsets_64.c"
        ));
        // `*` stays within one segment...
        assert!(!glob_match(
            "arch/*/kernel/asm-offsets*.c",
            "arch/x86/xen/kernel/asm-offsets.c"
        ));
        assert!(!glob_match("lib/*.c", "lib/vdso/gettimeofday.c"));
        // ...while `**` spans segments, including zero.
        assert!(glob_match("lib/vdso/**", "lib/vdso/gettimeofday.c"));
        assert!(glob_match("lib/vdso/**", "lib/vdso/deep/nested/file.c"));
        assert!(!glob_match("lib/vdso/**", "lib/vdso.c"));
        assert!(glob_match(
            "arch/*/kernel/vdso*/**",
            "arch/x86/kernel/vdso64/note.S"
        ));
        assert!(glob_match("**/*.c", "a/b/c.c"));
        assert!(glob_match("**/*.c", "top.c"));
        assert!(!glob_match("kernel/bounds.c", "kernel/bounds.c.orig"));
    }

    #[test]
    fn skeleton_tree_reduces_copies_and_links() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let src = tmp.path().join("src");
        let out = tmp.path().join("out");

        let write = |rel: &str, contents: &str| {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().expect("rel has parent")).expect("mkdir");
            std::fs::write(path, contents).expect("write fixture");
        };
        write("kernel/fork.c", "#include <a.h>\nint fork_body;\n");
        write("kernel/bounds.c", "int kept_whole;\n");
        write(
            "include/linux/a.h",
            "static inline int f(void) { return 1; }\n",
        );
        write("arch/x86/kernel/vmlinux.lds.S", "SECTIONS { }\n");
        write("scripts/mod/modpost.c", "int host_tool;\n");
        write("Makefile", "obj-y := fork.o\n");
        std::fs::create_dir_all(src.join("scripts/dtc/include-prefixes")).expect("mkdir");
        std::os::unix::fs::symlink(
            "../../../arch/x86/boot/dts",
            src.join("scripts/dtc/include-prefixes/x86"),
        )
        .expect("symlink fixture");

        skeleton(&src, &out, &[]).expect("skeleton");

        let read = |rel: &str| std::fs::read_to_string(out.join(rel)).expect("read output");
        assert_eq!(read("kernel/fork.c"), format!("{MARKER}#include <a.h>\n"));
        // Allowlisted, header, linker-script, host-tool, and build files are
        // byte-verbatim.
        assert_eq!(read("kernel/bounds.c"), "int kept_whole;\n");
        assert_eq!(
            read("include/linux/a.h"),
            "static inline int f(void) { return 1; }\n"
        );
        assert_eq!(read("arch/x86/kernel/vmlinux.lds.S"), "SECTIONS { }\n");
        assert_eq!(read("scripts/mod/modpost.c"), "int host_tool;\n");
        assert_eq!(read("Makefile"), "obj-y := fork.o\n");
        assert_eq!(
            std::fs::read_link(out.join("scripts/dtc/include-prefixes/x86"))
                .expect("symlink recreated"),
            std::path::PathBuf::from("../../../arch/x86/boot/dts")
        );
    }

    #[test]
    fn body_edits_reduce_identically() {
        // The whole mechanism: two trees differing only in bodies and
        // comments produce byte-identical skeletons.
        let before = "#include <a.h>\n// old comment\nint f(void) { return 1; }\n";
        let after = "#include <a.h>\n/* new comment */\nint f(void) { return 2; }\nint g(void);\n";
        assert_eq!(reduced(before, Lang::C), reduced(after, Lang::C));
    }

    #[test]
    fn deterministic_over_reruns() {
        let tmp = tempfile::tempdir().expect("create tempdir");
        let src = tmp.path().join("src");
        let write = |rel: &str, contents: &str| {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().expect("rel has parent")).expect("mkdir");
            std::fs::write(path, contents).expect("write fixture");
        };
        write("kernel/fork.c", "#include <a.h>\nint body;\n");
        write("kernel/sub/dir.c", "#define D 1\nint body;\n");
        write("Makefile", "obj-y := fork.o\n");

        let out_a = tmp.path().join("a");
        let out_b = tmp.path().join("b");
        skeleton(&src, &out_a, &[]).expect("first run");
        skeleton(&src, &out_b, &[]).expect("second run");

        let collect = |root: &Path| {
            let mut files = std::collections::BTreeMap::new();
            let mut stack = vec![root.to_path_buf()];
            while let Some(dir) = stack.pop() {
                for entry in std::fs::read_dir(&dir).expect("read output dir") {
                    let entry = entry.expect("dir entry");
                    if entry.file_type().expect("file type").is_dir() {
                        stack.push(entry.path());
                    } else {
                        let rel = entry
                            .path()
                            .strip_prefix(root)
                            .expect("under root")
                            .to_owned();
                        files.insert(rel, std::fs::read(entry.path()).expect("read file"));
                    }
                }
            }
            files
        };
        assert_eq!(collect(&out_a), collect(&out_b));
    }
}
