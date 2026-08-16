//! Intra-doc links, resolved against the interface.
//!
//! A doc comment written for the Rust surface reaches four published
//! surfaces. Rust's own link syntax travels with it, so before this module
//! existed a rustdoc link shipped into `index.d.ts` and the `.pyi` as text
//! that resolves to nothing, and a link naming an item that had since been
//! renamed shipped just as quietly: nothing connected the prose to the
//! identifier it named. Fourteen such lines survived a rename for weeks
//! (ENG-12396), and the guard that caught them was a denylist of one noun,
//! which the next rename would walk straight past.
//!
//! So links are resolved here instead, against the interface itself:
//!
//! ```text
//! [`Machine::forward_port`]   ->  {@link Machine.forwardPort}     (TypeScript)
//!                             ->  `Machine.forward_port`          (Python)
//! ```
//!
//! Two rules follow from that, and they are the whole contract:
//!
//! - **A link renders in the target language's own spelling.** The IR
//!   already carries every rename and the wire spelling of every variant,
//!   so `camelCase`, `SCREAMING_SNAKE_CASE` and `"wire-value"` come out of
//!   the same table that named the item in the first place.
//! - **A link that resolves to nothing is an error**, naming the doc site
//!   and the dead target, the way rustdoc's `broken_intra_doc_links` does
//!   at `deny`. This is what makes the stale-link class impossible rather
//!   than merely absent today: a rename that leaves prose behind stops the
//!   build that would have published it.
//!
//! Only the code-span spelling is a link. `` [`Machine`] `` and
//! `[the machine](Machine)` are resolved; a bare `[1]` in prose is prose,
//! and `[docs](https://ix.dev)` is a plain markdown link, because its
//! target is not an item path. The inline form's text is not carried into
//! the rendering: the target's language spelling replaces the whole link,
//! so write the sentence around the reference rather than through it.
//!
//! A reference link (`` [`Machine`][handle] ``) is refused rather than
//! passed over. Its target lives in a link definition, and nothing writes
//! those definitions into a `.d.ts` or a `.pyi`, so passing it through
//! ships exactly the dead text this module exists to refuse -- with no
//! build error, which is worse than the class it replaced.

use std::fmt;

use crate::casing;
use crate::ir;

/// Which language's spelling a doc comment renders for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    /// `TSDoc`: `{@link Machine.forwardPort}`, which editors resolve.
    Ts,
    /// A code span holding the Python name: `` `Machine.forward_port` ``.
    Py,
    /// A code span holding the Elixir name.
    Ex,
    /// A code span holding the JVM name.
    Jvm,
}

/// An unresolvable intra-doc link: which doc comment holds it, what it
/// names, and why nothing answered.
#[derive(Debug)]
pub struct DocError {
    /// How the interface names the item whose doc comment holds the link.
    pub site: String,
    /// The link target as written.
    pub link: String,
    /// Why it did not resolve, in the interface's own vocabulary.
    pub reason: String,
}

impl fmt::Display for DocError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "the doc comment on {site} links to `{link}`: {reason}",
            site = self.site,
            link = self.link,
            reason = self.reason,
        )
    }
}

impl std::error::Error for DocError {}

/// Every unresolvable link in one interface.
///
/// Reported together rather than one per build: a rename usually leaves
/// several behind, and finding them one compile at a time is the loop this
/// exists to shorten.
#[derive(Debug)]
pub struct DeadLinks {
    /// The dead links, in the order the interface declares their sites.
    pub links: Vec<DocError>,
}

impl fmt::Display for DeadLinks {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.links.len();
        let plural = if count == 1 { "link names" } else { "links name" };
        write!(
            formatter,
            "{count} intra-doc {plural} something this interface does not \
             declare. unibind resolves intra-doc links against the exported \
             surface and renders each one in the target language's spelling, so \
             a link that names nothing would ship into index.d.ts and the .pyi \
             as dead text. Name an exported item, or drop the brackets and \
             leave a plain code span for detail that stays on the Rust side."
        )?;
        for link in &self.links {
            write!(formatter, "\n  - {link}")?;
        }
        Ok(())
    }
}

impl std::error::Error for DeadLinks {}

/// Check every doc comment in `interface`: each intra-doc link must name an
/// item the interface declares.
///
/// The macro calls this at lowering, so a dead link fails the build that
/// would have generated from it rather than the publish that followed.
///
/// # Errors
///
/// Returns every unresolvable link, each naming its doc site and target.
pub fn validate(interface: &ir::Interface) -> Result<(), DeadLinks> {
    walk(interface, Mode::Check).map(drop)
}

/// `interface` with every doc comment rewritten into `language`'s spelling
/// of the items its links name.
///
/// # Errors
///
/// Returns every unresolvable link, each naming its doc site and target.
pub fn resolve(interface: &ir::Interface, language: Language) -> Result<ir::Interface, DeadLinks> {
    walk(interface, Mode::Render(language))
}

/// What a walk does with each link it finds.
#[derive(Clone, Copy)]
enum Mode {
    /// Resolve and discard: the interface comes back unchanged.
    Check,
    /// Resolve and rewrite in this language's spelling.
    Render(Language),
}

/// One resolved link target, carrying enough of the interface to spell it
/// in any language.
enum Target<'a> {
    Record(&'a ir::Record),
    Enumeration(&'a ir::Enum),
    Error(&'a ir::ErrorType),
    Object(&'a ir::Object),
    Function(&'a ir::Function),
    Method {
        object: &'a ir::Object,
        function: &'a ir::Function,
    },
    Constructor {
        object: &'a ir::Object,
    },
    Field {
        record: &'a ir::Record,
        field: &'a ir::Field,
    },
    Variant {
        declared: &'a ir::Enum,
        variant: &'a ir::EnumVariant,
    },
    ErrorVariant {
        variant: &'a ir::ErrorVariant,
    },
}

/// The type a `Self::` link resolves against: the item whose doc comment is
/// being read.
#[derive(Clone, Copy)]
enum Owner<'a> {
    /// A module-level doc comment, where `Self` names nothing.
    None,
    Record(&'a ir::Record),
    Enumeration(&'a ir::Enum),
    Error(&'a ir::ErrorType),
    Object(&'a ir::Object),
}

impl<'a> Owner<'a> {
    /// The type `Self` names at this doc site, for a bare `` [`Self`] ``.
    const fn target(self) -> Option<Target<'a>> {
        Some(match self {
            Self::None => return None,
            Self::Record(record) => Target::Record(record),
            Self::Enumeration(declared) => Target::Enumeration(declared),
            Self::Error(error) => Target::Error(error),
            Self::Object(object) => Target::Object(object),
        })
    }

    /// The type name a `Self::x` link resolves through; `None` at a doc
    /// site that has no enclosing type.
    fn type_name(self) -> Option<&'a str> {
        match self {
            Self::None => None,
            Self::Record(record) => Some(&record.name),
            Self::Enumeration(declared) => Some(&declared.name),
            Self::Error(error) => Some(&error.name),
            Self::Object(object) => Some(&object.name),
        }
    }
}

/// One link occurrence in a doc line.
struct Parsed<'a> {
    /// The target path as written.
    path: &'a str,
    /// Bytes of the line the whole link occupies, from its `[`.
    len: usize,
    /// Which markdown spelling it uses.
    form: Form,
}

/// The markdown spellings a link can take.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Form {
    /// `` [`Machine`] `` or `[text](Machine)`: the target is right there.
    Direct,
    /// `` [`Machine`][handle] ``: the target is a label defined elsewhere in
    /// the document. Refused, because the generated surface has nowhere to
    /// put the definition, so it would ship as dead text.
    Reference,
}

/// Parse the link that starts at `text`, which begins with `[`.
///
/// `None` for a bracket that is not an intra-doc link: prose (`[1]`), or a
/// plain markdown link whose target is a URL rather than an item path. The
/// code-span form is always a link claim, so a target that is not an item
/// path stays a link here and fails in resolution, where the message can
/// say so -- and so does a reference link, whose definition the generated
/// surface has no place for.
fn parse_link(text: &str) -> Option<Parsed<'_>> {
    let code_span = text.starts_with("[`");
    let (inner, bracket_end) = if code_span {
        let rest = text.get(2..)?;
        let end = rest.find("`]")?;
        (rest.get(..end)?, 2 + end + 2)
    } else {
        let rest = text.get(1..)?;
        let end = rest.find(']')?;
        (rest.get(..end)?, 1 + end + 1)
    };
    let tail = text.get(bracket_end..)?;
    if let Some(target) = tail.strip_prefix('(') {
        let end = target.find(')')?;
        let path = target.get(..end)?;
        if !is_item_path(path) {
            // A real markdown link: `[the docs](https://ix.dev)`.
            return None;
        }
        return Some(Parsed {
            path,
            len: bracket_end + 1 + end + 1,
            form: Form::Direct,
        });
    }
    if !code_span {
        return None;
    }
    // A reference link carries its target in a definition elsewhere in the
    // document. Nothing renders those definitions into a `.d.ts` or a `.pyi`,
    // so the reference would ship as the text it is written with -- exactly
    // the dead text this module exists to refuse. It is a link claim, so it
    // is caught rather than passed over.
    if let Some(label) = tail.strip_prefix('[') {
        let end = label.find(']')?;
        return Some(Parsed {
            path: inner,
            len: bracket_end + 1 + end + 1,
            form: Form::Reference,
        });
    }
    Some(Parsed {
        path: inner,
        len: bracket_end,
        form: Form::Direct,
    })
}

/// Whether `path` is shaped like a Rust item path: `Type`, `Type::member`,
/// `Self::member`, with an optional `()` after a function name.
fn is_item_path(path: &str) -> bool {
    let path = path.strip_suffix("()").unwrap_or(path);
    !path.is_empty()
        && path.split("::").all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
                && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
}

/// Resolve `path` against `interface`, with `Self` naming `owner`.
fn resolve_path<'a>(
    interface: &'a ir::Interface,
    owner: Owner<'a>,
    path: &str,
) -> Result<Target<'a>, String> {
    let path = path.strip_suffix("()").unwrap_or(path);
    if !is_item_path(path) {
        return Err(format!(
            "`{path}` is not an item path; write it in a plain code span if it \
             is not a link"
        ));
    }
    let mut segments = path.split("::");
    let (first, second, rest) = (segments.next(), segments.next(), segments.next());
    let Some(first) = first else {
        return Err("the link is empty".to_owned());
    };
    if rest.is_some() {
        return Err(
            "an intra-doc link names an item of this interface, not a path \
             into another crate or module"
                .to_owned(),
        );
    }
    let Some(member) = second else {
        if first == "Self" {
            return owner.target().ok_or_else(|| {
                "`Self` names nothing at this doc site, which has no enclosing \
                 type; write the type's name"
                    .to_owned()
            });
        }
        return resolve_type_or_function(interface, first);
    };
    let type_name = if first == "Self" {
        owner.type_name().ok_or_else(|| {
            "`Self` names nothing at this doc site, which has no enclosing type"
                .to_owned()
        })?
    } else {
        first
    };
    resolve_member(interface, type_name, member)
}

/// Resolve a single-segment link: a declared type or an exported free
/// function.
fn resolve_type_or_function<'a>(
    interface: &'a ir::Interface,
    name: &str,
) -> Result<Target<'a>, String> {
    if let Some(record) = find(&interface.records, name, |record| &record.name) {
        return Ok(Target::Record(record));
    }
    if let Some(declared) = find(&interface.enums, name, |declared| &declared.name) {
        return Ok(Target::Enumeration(declared));
    }
    if let Some(error) = find(&interface.errors, name, |error| &error.name) {
        return Ok(Target::Error(error));
    }
    if let Some(object) = find(&interface.objects, name, |object| &object.name) {
        return Ok(Target::Object(object));
    }
    if let Some(function) = find(&interface.functions, name, |function| &function.name) {
        return Ok(Target::Function(function));
    }
    Err(format!(
        "no record, enumeration, error, object, or exported function is named \
         `{name}`"
    ))
}

/// Resolve a `Type::member` link.
fn resolve_member<'a>(
    interface: &'a ir::Interface,
    type_name: &str,
    member: &str,
) -> Result<Target<'a>, String> {
    if let Some(object) = find(&interface.objects, type_name, |object| &object.name) {
        if let Some(function) = find(&object.methods, member, |function| &function.name)
            .or_else(|| find(&object.associated, member, |function| &function.name))
        {
            return Ok(Target::Method { object, function });
        }
        if object
            .constructor
            .as_ref()
            .is_some_and(|constructor| constructor.name == member)
        {
            return Ok(Target::Constructor { object });
        }
        return Err(format!(
            "`{type_name}` declares no method, associated function, or \
             constructor named `{member}`"
        ));
    }
    if let Some(record) = find(&interface.records, type_name, |record| &record.name) {
        let Some(field) = find(&record.fields, member, |field| &field.name) else {
            return Err(format!("`{type_name}` has no field `{member}`"));
        };
        return Ok(Target::Field { record, field });
    }
    if let Some(declared) = find(&interface.enums, type_name, |declared| &declared.name) {
        let Some(variant) = find(&declared.variants, member, |variant| &variant.name) else {
            return Err(format!("`{type_name}` has no variant `{member}`"));
        };
        return Ok(Target::Variant { declared, variant });
    }
    if let Some(error) = find(&interface.errors, type_name, |error| &error.name) {
        let Some(variant) = find(&error.variants, member, |variant| &variant.name) else {
            return Err(format!("`{type_name}` has no variant `{member}`"));
        };
        return Ok(Target::ErrorVariant { variant });
    }
    Err(format!(
        "no record, enumeration, error, or object is named `{type_name}`"
    ))
}

/// The first item of `items` whose `name` matches.
fn find<'a, T>(items: &'a [T], name: &str, name_of: impl Fn(&T) -> &String) -> Option<&'a T> {
    items.iter().find(|item| name_of(item) == name)
}

/// A rendered reference, before the language decides how to mark it up.
enum Reference {
    /// An identifier the target language declares, which `TSDoc` can link.
    Declared(String),
    /// A value with no declared identifier to link, such as a TypeScript
    /// union member, which every language renders as a code span.
    Value(String),
}

/// The target's spelling in `language`, marked up the way that language's
/// tooling reads references.
fn render_target(target: &Target<'_>, language: Language) -> String {
    match (language, reference(target, language)) {
        (Language::Ts, Reference::Declared(text)) => format!("{{@link {text}}}"),
        (_, Reference::Declared(text) | Reference::Value(text)) => format!("`{text}`"),
    }
}

/// How `language` spells the target.
fn reference(target: &Target<'_>, language: Language) -> Reference {
    match *target {
        Target::Record(record) => Reference::Declared(type_name(language, &record.names, &record.name)),
        Target::Enumeration(declared) => {
            Reference::Declared(type_name(language, &declared.names, &declared.name))
        }
        Target::Error(error) => Reference::Declared(type_name(language, &error.names, &error.name)),
        Target::Object(object) => Reference::Declared(type_name(language, &object.names, &object.name)),
        Target::Function(function) => {
            Reference::Declared(value_name(language, &function.names, &function.name))
        }
        Target::Method { object, function } => Reference::Declared(format!(
            "{}.{}",
            type_name(language, &object.names, &object.name),
            value_name(language, &function.names, &function.name)
        )),
        // A constructor has no name of its own in any target language: it
        // is `new Machine(...)` and `Machine(...)`, so the type is the
        // reference.
        Target::Constructor { object } => {
            Reference::Declared(type_name(language, &object.names, &object.name))
        }
        Target::Field { record, field } => Reference::Declared(format!(
            "{}.{}",
            type_name(language, &record.names, &record.name),
            value_name(language, &field.names, &field.name)
        )),
        Target::Variant { declared, variant } => variant_reference(language, declared, variant),
        // Every backend renders an error variant as its own class beside
        // the base, so the variant name is the whole reference.
        Target::ErrorVariant { variant } => {
            Reference::Declared(type_name(language, &variant.names, &variant.name))
        }
    }
}

/// An enumeration variant, which is the one target whose reference is not
/// an identifier in every language: a TypeScript union member is the wire
/// string itself, and an Elixir one is an atom.
fn variant_reference(
    language: Language,
    declared: &ir::Enum,
    variant: &ir::EnumVariant,
) -> Reference {
    match language {
        Language::Ts => Reference::Value(format!("\"{}\"", variant.wire)),
        Language::Ex => Reference::Value(format!(":{}", variant.wire)),
        Language::Py | Language::Jvm => Reference::Value(format!(
            "{}.{}",
            type_name(language, &declared.names, &declared.name),
            member_name(language, variant)
        )),
    }
}

/// A variant's member identifier in the languages that have one. Lowering
/// fills `names.py` with the `SCREAMING_SNAKE_CASE` default, so the
/// fallback only covers IR some other producer wrote and applies the same
/// rule rather than a second one.
fn member_name(language: Language, variant: &ir::EnumVariant) -> String {
    let declared = match language {
        Language::Ts => variant.names.ts.as_deref(),
        Language::Py => variant.names.py.as_deref(),
        Language::Ex => variant.names.ex.as_deref(),
        Language::Jvm => variant.names.jvm.as_deref(),
    };
    declared.map_or_else(
        || casing::screaming_snake_case(&variant.name),
        ToOwned::to_owned,
    )
}

/// The target language's name for a declared type: the rename when set,
/// the Rust name otherwise. The rule every backend already applies.
fn type_name(language: Language, names: &ir::Names, rust: &str) -> String {
    rename(language, names).unwrap_or(rust).to_owned()
}

/// The target language's name for a value (function, method, or field):
/// the rename when set, else the language's own convention for the Rust
/// name. `camelCase` is napi's conversion, which the JVM backend's Java
/// naming matches; Python and Elixir keep `snake_case`.
fn value_name(language: Language, names: &ir::Names, rust: &str) -> String {
    if let Some(renamed) = rename(language, names) {
        return renamed.to_owned();
    }
    match language {
        Language::Ts | Language::Jvm => casing::lower_camel_case(rust),
        Language::Py | Language::Ex => rust.to_owned(),
    }
}

/// The rename `language` declared for an item, if any.
fn rename(language: Language, names: &ir::Names) -> Option<&str> {
    match language {
        Language::Ts => names.ts.as_deref(),
        Language::Py => names.py.as_deref(),
        Language::Ex => names.ex.as_deref(),
        Language::Jvm => names.jvm.as_deref(),
    }
}

/// Rewrite one doc line, resolving every link it holds.
fn rewrite_line(line: &str, context: &Context<'_>, dead: &mut Vec<DocError>) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('[') {
        let (before, tail) = rest.split_at(open);
        out.push_str(before);
        let Some(parsed) = parse_link(tail) else {
            out.push('[');
            rest = tail.get(1..).unwrap_or_default();
            continue;
        };
        let written = tail.get(..parsed.len).unwrap_or_default();
        let resolved = if parsed.form == Form::Reference {
            Err(format!(
                "a reference link is resolved through a link definition, and \
                 nothing writes those definitions into index.d.ts or the .pyi, \
                 so this would ship as the text it is written with. Write it \
                 inline: [`{}`]",
                parsed.path
            ))
        } else {
            resolve_path(context.interface, context.owner, parsed.path)
        };
        match resolved {
            // A dead link stays as written while the walk goes on, so one
            // build reports every one of them rather than the first.
            Err(reason) => {
                dead.push(DocError {
                    site: context.site.to_owned(),
                    link: parsed.path.to_owned(),
                    reason,
                });
                out.push_str(written);
            }
            Ok(target) => match context.mode {
                Mode::Check => out.push_str(written),
                Mode::Render(language) => out.push_str(&render_target(&target, language)),
            },
        }
        rest = tail.get(parsed.len..).unwrap_or_default();
    }
    out.push_str(rest);
    out
}

/// One doc site being walked: what its links resolve against, what `Self`
/// names there, how the diagnostic names it, and what to do with each link.
struct Context<'a> {
    interface: &'a ir::Interface,
    owner: Owner<'a>,
    site: &'a str,
    mode: Mode,
}

/// Rewrite every line of one doc comment.
fn rewrite_docs(docs: &mut [String], context: &Context<'_>, dead: &mut Vec<DocError>) {
    for line in docs {
        *line = rewrite_line(line, context, dead);
    }
}

/// How a diagnostic names a doc site: `` `exec` `` for a free function,
/// `` `Machine::exec` `` for anything declared on a type.
fn site_name(owner: Option<&str>, name: &str) -> String {
    owner.map_or_else(
        || format!("`{name}`"),
        |owner| format!("`{owner}::{name}`"),
    )
}

/// Resolve the links in every doc comment of `interface`, returning the
/// interface `mode` produced.
fn walk(interface: &ir::Interface, mode: Mode) -> Result<ir::Interface, DeadLinks> {
    let mut out = interface.clone();
    let mut dead = Vec::new();
    rewrite_docs(
        &mut out.docs,
        &Context {
            interface,
            owner: Owner::None,
            site: "the exported module",
            mode,
        },
        &mut dead,
    );
    for (rendered, function) in out.functions.iter_mut().zip(&interface.functions) {
        let site = site_name(None, &function.name);
        rewrite_docs(
            &mut rendered.docs,
            &Context {
                interface,
                owner: Owner::None,
                site: &site,
                mode,
            },
            &mut dead,
        );
    }
    for (rendered, record) in out.records.iter_mut().zip(&interface.records) {
        let owner = Owner::Record(record);
        let site = site_name(None, &record.name);
        rewrite_docs(
            &mut rendered.docs,
            &Context {
                interface,
                owner,
                site: &site,
                mode,
            },
            &mut dead,
        );
        for (rendered, field) in rendered.fields.iter_mut().zip(&record.fields) {
            let site = site_name(Some(&record.name), &field.name);
            rewrite_docs(
                &mut rendered.docs,
                &Context {
                    interface,
                    owner,
                    site: &site,
                    mode,
                },
                &mut dead,
            );
        }
    }
    walk_enums(&mut out, interface, mode, &mut dead);
    walk_errors(&mut out, interface, mode, &mut dead);
    walk_objects(&mut out, interface, mode, &mut dead);
    if dead.is_empty() {
        return Ok(out);
    }
    Err(DeadLinks { links: dead })
}

/// The enumeration half of [`walk`]: the type's own docs and each
/// variant's.
fn walk_enums(
    out: &mut ir::Interface,
    interface: &ir::Interface,
    mode: Mode,
    dead: &mut Vec<DocError>,
) {
    for (rendered, declared) in out.enums.iter_mut().zip(&interface.enums) {
        let owner = Owner::Enumeration(declared);
        let site = site_name(None, &declared.name);
        rewrite_docs(
            &mut rendered.docs,
            &Context {
                interface,
                owner,
                site: &site,
                mode,
            },
            dead,
        );
        for (rendered, variant) in rendered.variants.iter_mut().zip(&declared.variants) {
            let site = site_name(Some(&declared.name), &variant.name);
            rewrite_docs(
                &mut rendered.docs,
                &Context {
                    interface,
                    owner,
                    site: &site,
                    mode,
                },
                dead,
            );
        }
    }
}

/// The error half of [`walk`].
fn walk_errors(
    out: &mut ir::Interface,
    interface: &ir::Interface,
    mode: Mode,
    dead: &mut Vec<DocError>,
) {
    for (rendered, error) in out.errors.iter_mut().zip(&interface.errors) {
        let owner = Owner::Error(error);
        let site = site_name(None, &error.name);
        rewrite_docs(
            &mut rendered.docs,
            &Context {
                interface,
                owner,
                site: &site,
                mode,
            },
            dead,
        );
        for (rendered, variant) in rendered.variants.iter_mut().zip(&error.variants) {
            let site = site_name(Some(&error.name), &variant.name);
            rewrite_docs(
                &mut rendered.docs,
                &Context {
                    interface,
                    owner,
                    site: &site,
                    mode,
                },
                dead,
            );
        }
    }
}

/// The object half of [`walk`]: the handle's own docs, its constructor,
/// its associated functions, and its methods.
fn walk_objects(
    out: &mut ir::Interface,
    interface: &ir::Interface,
    mode: Mode,
    dead: &mut Vec<DocError>,
) {
    for (rendered, object) in out.objects.iter_mut().zip(&interface.objects) {
        let owner = Owner::Object(object);
        let site = site_name(None, &object.name);
        rewrite_docs(
            &mut rendered.docs,
            &Context {
                interface,
                owner,
                site: &site,
                mode,
            },
            dead,
        );
        let members = rendered
            .constructor
            .iter_mut()
            .chain(rendered.associated.iter_mut())
            .chain(rendered.methods.iter_mut());
        for member in members {
            let site = site_name(Some(&object.name), &member.name);
            rewrite_docs(
                &mut member.docs,
                &Context {
                    interface,
                    owner,
                    site: &site,
                    mode,
                },
                dead,
            );
        }
    }
}
