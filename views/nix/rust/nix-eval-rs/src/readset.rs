//! Recording what an evaluation read, and memoising its result against that.
//!
//! # Why a read set can be trusted here
//!
//! `maintainers/ix/read-set-recall.md` is a record of read-set invalidation
//! going wrong in the C++ evaluator, so a read set arriving in this repo owes
//! an argument for why it is different. The argument is that this evaluator
//! has exactly one way to reach the world. Every question it can ask is a
//! [`Host`] call (`builtins.getEnv` included, since it stopped calling
//! `std::env::var` behind the trait's back), and `builtins.purity_tests`
//! fails the build if an impure builtin is implemented that does not go
//! through it. So a recording `Host` sees everything, by construction rather
//! than by having looked hard.
//!
//! # Why replaying a read set is sound
//!
//! The evaluator is deterministic: its result is a function of the module and
//! the answers it received, and so is the *next question it asks*. That second
//! half is what makes verified replay work.
//!
//! A memoised result is stored under `H(module, questions and answers, in
//! order)`. To look one up, the recorded question list from last time is
//! replayed against the host now, and the key is computed from the answers
//! *observed now*, never from the recorded ones. If a result exists under that
//! key, some past evaluation asked exactly this sequence and got exactly these
//! answers; determinism then says the evaluation being asked for would ask the
//! same questions, receive the same answers, and produce the same result.
//!
//! The consequence worth stating: **a stale witness cannot produce a wrong
//! answer, only a miss.** If the recorded question list no longer matches what
//! the evaluation would ask, the key computed from replaying it is a key no
//! evaluation ever stored a result under, so the lookup misses and the
//! evaluation runs. The witness is a hint about which questions to ask, and
//! correctness does not rest on it being right.

use crate::host::{FileType, Host, LookupError, StoreError};
use crate::task::SearchPathEntry;
use ix_kernel::hash::{self, Hash};
use std::cell::RefCell;

/// Domain separation for read-set digests.
const READ_TAG: &str = "ixe-read-v1";
/// Domain separation for the composed evaluation key.
const EVAL_TAG: &str = "ixe-eval-result-v1";

/// One question, without its answer. This is what gets remembered so it can
/// be asked again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    ReadFile(String),
    /// The same file as [`Question::ReadFile`], read as raw bytes: what
    /// `builtins.hashFile` records. A question of its own rather than a
    /// second recording under `ReadFile`, because each question kind owns
    /// one answer encoding, and this one digests the bytes where `ReadFile`
    /// digests the string answer. On a non-UTF-8 file those differ, and one
    /// key carrying two digests would invalidate itself.
    ReadFileBytes(String),
    ReadDir(String),
    PathExists(String),
    FileType(String),
    /// `builtins.readFileType`'s question asked the other way: the type of
    /// the path with every symlink in it resolved.
    ///
    /// A question of its own and not a duplicate of [`Question::FileType`],
    /// because the two have different answers for the same path and an
    /// `import` asks this one. Recording it as `FileType` would replay the
    /// `lstat`, get `"symlink"` where the evaluation saw `"directory"`, and
    /// compute a key no result was ever stored under -- a permanent miss for
    /// every expression importing through a symlink. ENG-12871.
    FileTypeResolved(String),
    GetEnv(String),
    /// A path copied into the store by a string coercion. A question rather
    /// than a bystander because the answer is a content hash of the file: an
    /// edit to it changes the store path, so an evaluation that interpolated
    /// it must miss afterwards.
    CopyToStore(String),
    /// A search path lookup: which entries were walked and what was sought.
    /// The entries are part of the question because the same `<x>` resolves
    /// differently under a different `-I`, and the answer is the file that
    /// won, so a new entry shadowing the old one misses.
    FindFile {
        entries: Vec<SearchPathEntry>,
        name: String,
    },
    /// The default search path itself. Asked whenever `__nixPath` is
    /// evaluated, which `<x>` does, so a change to `-I` invalidates every
    /// result that looked anything up even when the file it found is
    /// unchanged.
    NixPath,
    /// Text `builtins.toFile` asked the store to hold.
    ///
    /// The store *path* is a pure function of the name, the bytes and the
    /// references, so it cannot change under a fixed memo key. Recorded anyway,
    /// for the reason `EnsurePath` is: whether the bytes were actually written
    /// depends on the embedder's `readOnlyMode` and on the store still having
    /// them, so a memoised success could otherwise be served to a run whose
    /// store never wrote the file and whose caller then expects it to be there.
    StoreText {
        name: String,
        contents: String,
        references: Vec<String>,
    },
    /// A string context an evaluation realised before reading through it:
    /// import from derivation.
    ///
    /// **This is the read-set entry that makes a memoised IFD sound.** The
    /// answer is a rewrite map, which is empty for every input-addressed
    /// derivation, so it is emphatically not the interesting part of the
    /// digest. What is interesting is that the question was *asked at all*
    /// and that asking it succeeded: the outcome depends on whether the store
    /// can still produce those outputs, which is a fact about the world and
    /// not about the expression. Left unrecorded, a result memoised on a
    /// machine that built the derivation would be served, later, to a run
    /// whose store had been garbage-collected -- and the reads keyed beside
    /// it would replay against paths that are gone.
    ///
    /// That is the same argument [`Question::EnsurePath`] makes, and it is
    /// stronger here, because a `Realise` can *cause* a build. Replaying one
    /// re-runs it, so a witness naming this question is a witness whose
    /// replay is not free. It is recorded anyway: a cheap wrong answer is
    /// worse than an expensive right one, and in the common case the outputs
    /// are already valid and `buildPaths` returns without doing anything.
    Realise(Vec<crate::value2::ContextElem>),
    /// A store path `builtins.appendContext` asked to be made present.
    ///
    /// A question for the same reason `CopyToStore` is: whether the store can
    /// produce the path decides whether the evaluation succeeds, so it is a
    /// fact about the world at the time it ran. It was forwarded and not
    /// recorded before, so a memoised success could be served to a run whose
    /// store had since lost the path.
    ///
    /// Tag 9 rather than the next small number: 7 and 8 belong to the search
    /// path questions, and a tag is a wire format, so reusing one would make
    /// an old witness decode as a different question.
    EnsurePath(String),
    /// A filtered copy `builtins.path` asked the store to make.
    ///
    /// The whole request is the key, not just the root: the name, the
    /// ingestion method, the expected hash and every accepted path change what
    /// gets archived and so change the store path. The *answer* is the store
    /// path, which is a content hash of the bytes copied -- and those bytes
    /// are the one thing the walk never read, so nothing else in a read set
    /// notices an edit inside an accepted file. This question is what does.
    ///
    /// A witness for a large filtered tree is correspondingly large: one entry
    /// per accepted path. That is the honest size of the question, and
    /// shrinking it to a digest would make a replayed witness unable to re-ask
    /// it.
    StoreFiltered(Box<crate::task::FilteredCopy>),
    /// A URL `builtins.fetchurl` or `builtins.fetchTarball` asked the store
    /// to hold.
    ///
    /// Recorded for two reasons at once, and either alone would be enough.
    /// An *unpinned* fetch is the most impure question in the language: the
    /// bytes behind a URL change with nobody's permission, so a memoised
    /// result keyed on anything less than "what did this URL answer" is a
    /// stale download served as a fresh one. A *pinned* fetch has a store
    /// path that is a pure function of the request -- and whether the store
    /// can still produce it is not, which is exactly the fact
    /// [`Question::EnsurePath`] exists to record.
    Fetch(Box<crate::task::FetchRequest>),
    /// A tree `builtins.fetchTree` or `builtins.fetchGit` asked for.
    ///
    /// The most impure question here after an unpinned [`Question::Fetch`],
    /// and impure in a second way that one is not: a `git` input with a `ref`
    /// and no `rev` resolves to whatever that branch points at *now*, so the
    /// same question answers differently between two runs a commit apart. The
    /// answer -- the whole emitted attribute set, digested -- is the only
    /// thing in the key that notices.
    FetchTree(Box<crate::task::FetchTreeRequest>),
    /// A flake reference `builtins.getFlake` asked the embedder to lock.
    ///
    /// Recorded, and the recording is what makes a memoised `getFlake` sound
    /// rather than merely fast. The flake reference alone does not determine
    /// the answer: `lockFlake` consults the registry, walks the input graph
    /// and reads a `flake.lock` that can change under a fixed reference, so
    /// two runs a commit apart can lock `path:/src/x` to two different trees.
    /// What notices is the digest of the *answer* -- the lock file and the
    /// overrides document -- which replay recomputes by asking again. A run
    /// whose lock moved gets a different digest, a different key, and a fresh
    /// evaluation.
    ///
    /// The alternative, leaving the question unrecorded, is the ENG-12540
    /// shape: the memoised outputs of yesterday's lock served to today's
    /// reference, with nothing in the key that could tell.
    LockFlake(String),
    /// A flake reference `builtins.parseFlakeRef` asked the embedder to
    /// explode.
    ///
    /// Nearly pure -- the grammar is fixed -- but not a function of the
    /// string alone: the answer can turn on the embedder's fetch settings,
    /// and the feature gate behind the hook decides between an answer and an
    /// error. Neither is in the settings fingerprint, so the question records
    /// itself and the digest of the answer is what notices.
    ParseFlakeRef(String),
    /// An attribute set `builtins.flakeRefToString` asked the embedder to
    /// print. Recorded for the reason [`Question::ParseFlakeRef`] is; the
    /// whole bag is the key, tagged like [`Question::FetchTree`]'s and for
    /// the same reason.
    FlakeRefToString(std::collections::BTreeMap<String, crate::task::TreeAttr>),
}

impl Question {
    /// The host question this was recorded from.
    ///
    /// A `Question` is a `NeedPath` that has been through a read set, and the
    /// two carry the same payload, so this is a rename rather than a
    /// translation. It exists so the purity policy can be read through a
    /// recorded question without a second copy of the table: replay asks the
    /// same things a live evaluation asks, so it must obey the same rules, and
    /// two tables would be two chances to disagree about what `pure-eval`
    /// permits.
    ///
    /// `Warn` and `Trace` have no `Question`: they are outputs, replayed from
    /// [`Emission`] instead. An import is recorded as the two questions it
    /// performs, [`Question::FileTypeResolved`] and then
    /// [`Question::ReadFile`].
    #[must_use]
    pub fn as_need_path(&self) -> crate::task::NeedPath {
        use crate::task::NeedPath;
        match self {
            Self::ReadFile(p) => NeedPath::Contents(p.clone()),
            // `Contents` and not `HashFile`: the purity table is the only
            // consumer, the two share its filesystem-read arm, and the
            // algorithm is not recorded here -- the digest encoding, not the
            // verdict, is what tells the questions apart. The same reasoning
            // as `FileType` below.
            Self::ReadFileBytes(p) => NeedPath::Contents(p.clone()),
            Self::ReadDir(p) => NeedPath::Entries(p.clone()),
            Self::PathExists(p) => NeedPath::Exists(p.clone()),
            // `Kind` and not `MaybeKind`: one recorded question, because
            // one host method answers both and the read set records the
            // read, not which contract asked for it. They share a purity arm
            // (`purity.rs`), so the choice changes no verdict; `Kind` is the
            // spelling because it is the question this `Question` is named
            // after. The same reasoning as `FileTypeResolved` below.
            Self::FileType(p) => NeedPath::Kind(p.clone()),
            // `Import` and not `Kind`, because this is the question an import
            // asks and the purity table has to see it as one. The two share
            // an arm there (`purity.rs:214`), so today the choice changes no
            // verdict; naming the real question is what keeps that true if
            // they ever stop sharing.
            Self::FileTypeResolved(p) => NeedPath::Import(p.clone()),
            Self::GetEnv(n) => NeedPath::Env(n.clone()),
            Self::CopyToStore(p) => NeedPath::StorePath(p.clone()),
            Self::FindFile { entries, name } => NeedPath::FindFile {
                entries: entries.clone(),
                name: name.clone(),
            },
            Self::NixPath => NeedPath::NixPath,
            Self::EnsurePath(p) => NeedPath::EnsurePath(p.clone()),
            Self::Realise(context) => NeedPath::Realise(context.clone()),
            Self::StoreText {
                name,
                contents,
                references,
            } => NeedPath::StoreText {
                name: name.clone(),
                contents: contents.clone(),
                references: references.clone(),
            },
            Self::StoreFiltered(r) => NeedPath::StoreFiltered(r.clone()),
            Self::Fetch(r) => NeedPath::Fetch(r.clone()),
            Self::FetchTree(r) => NeedPath::FetchTree(r.clone()),
            Self::LockFlake(r) => NeedPath::Flake(r.clone()),
            Self::ParseFlakeRef(r) => NeedPath::ParseFlakeRef(r.clone()),
            Self::FlakeRefToString(a) => NeedPath::FlakeRefToString(a.clone()),
        }
    }

    /// A stable tag per variant, written out rather than derived from
    /// position, so reordering the enum cannot silently change a key.
    const fn tag(&self) -> u8 {
        match self {
            Self::ReadFile(_) => 1,
            Self::ReadDir(_) => 2,
            Self::PathExists(_) => 3,
            Self::FileType(_) => 4,
            Self::GetEnv(_) => 5,
            Self::CopyToStore(_) => 6,
            Self::FindFile { .. } => 7,
            Self::NixPath => 8,
            Self::EnsurePath(_) => 9,
            Self::StoreText { .. } => 10,
            Self::StoreFiltered(_) => 11,
            Self::Fetch(_) => 12,
            Self::FetchTree(_) => 13,
            // The next free number, not one wedged in beside `FileType`: a
            // tag is stable, and renumbering the ones after it would make
            // every witness on disk decode to the wrong question.
            Self::FileTypeResolved(_) => 14,
            Self::LockFlake(_) => 15,
            Self::Realise(_) => 16,
            Self::ReadFileBytes(_) => 17,
            Self::ParseFlakeRef(_) => 18,
            Self::FlakeRefToString(_) => 19,
        }
    }

    /// The single string this question is about, for the digest. The search
    /// path questions carry more than one string, so they contribute their
    /// own parts in [`Question::key_parts`] and answer the empty string here.
    fn arg(&self) -> &str {
        match self {
            Self::ReadFile(a)
            | Self::ReadFileBytes(a)
            | Self::ReadDir(a)
            | Self::PathExists(a)
            | Self::FileType(a)
            | Self::FileTypeResolved(a)
            | Self::GetEnv(a)
            | Self::CopyToStore(a) => a,
            Self::FindFile { name, .. } => name,
            Self::NixPath => "",
            Self::EnsurePath(path) => path,
            Self::StoreText { name, .. } => name,
            Self::StoreFiltered(r) => &r.root,
            Self::Fetch(r) => &r.url,
            Self::LockFlake(r) => r,
            Self::ParseFlakeRef(r) => r,
            // No single string is the subject: the whole bag is. `key_parts`
            // carries it, and the empty arg is what `NixPath` does for the
            // same reason.
            Self::FetchTree(_) | Self::Realise(_) | Self::FlakeRefToString(_) => "",
        }
    }

    /// Everything beyond the tag and [`Question::arg`] that identifies this
    /// question. Empty for every question whose argument is one string.
    ///
    /// A search path lookup is keyed on the entries as well as the name,
    /// because they are an argument the program supplies: `findFile` takes
    /// the list, and two lookups of the same name against different lists are
    /// two different questions with two different answers.
    fn key_parts(&self) -> Vec<Vec<u8>> {
        match self {
            Self::FindFile { entries, .. } => entries
                .iter()
                .flat_map(|e| [e.prefix.clone().into_bytes(), e.path.clone().into_bytes()])
                .collect(),
            // The bytes and the references are arguments too: the same name
            // with different contents is a different file at a different
            // path, so keying on the name alone would replay one as the other.
            Self::StoreText {
                contents,
                references,
                ..
            } => {
                let mut parts = vec![contents.clone().into_bytes()];
                parts.extend(references.iter().map(|r| r.clone().into_bytes()));
                parts
            }
            // Everything but the root, which `arg` already contributes. The
            // markers keep "no filtering" apart from "a filter that accepted
            // nothing", and an absent `sha256` apart from one whose value is
            // the empty string: both pairs are different requests with
            // different answers.
            Self::StoreFiltered(r) => {
                let mut parts = vec![
                    r.name.clone().into_bytes(),
                    r.method.as_str().as_bytes().to_vec(),
                ];
                match &r.expected_sha256 {
                    Some(h) => {
                        parts.push(b"sha256".to_vec());
                        parts.push(h.clone().into_bytes());
                    }
                    None => parts.push(b"no-sha256".to_vec()),
                }
                match &r.accepted {
                    None => parts.push(b"unfiltered".to_vec()),
                    Some(list) => {
                        parts.push(b"filtered".to_vec());
                        for e in list {
                            parts.push(e.path.clone().into_bytes());
                            parts.push(e.file_type.as_str().as_bytes().to_vec());
                        }
                    }
                }
                // Pushed only when set, which is not a shortcut: a witness
                // recorded before this field existed described a copy that
                // did not inherit references, so the false case has to key
                // exactly as it did then or every one of them misses at once.
                // A `false` marker here would be correct and would also
                // invalidate the whole recorded corpus for nothing.
                if r.inherit_references {
                    parts.push(b"inherit-references".to_vec());
                }
                parts
            }
            // Everything but the URL, which `arg` already contributes. The
            // name is part of the store path, the kind decides both the
            // ingestion method and what is downloaded, and the marker keeps
            // an absent `sha256` apart from one whose value happens to be
            // the all-zero hash -- the first is an unpinned fetch and the
            // second is a pinned one that will fail its check.
            Self::Fetch(r) => {
                let mut parts = vec![
                    r.name.clone().into_bytes(),
                    r.kind.as_str().as_bytes().to_vec(),
                ];
                match &r.expected_sha256 {
                    Some(h) => {
                        parts.push(b"sha256".to_vec());
                        parts.push(h.clone().into_bytes());
                    }
                    None => parts.push(b"no-sha256".to_vec()),
                }
                parts
            }
            // Every element, in the order the evaluator sent them, rendered
            // the one way cppnix renders them. A context is a set and this
            // list came out of a `BTreeSet`, so the order is a function of
            // the elements rather than of how the string was built -- which
            // is what stops two evaluations that concatenated the same two
            // derivation outputs in opposite orders from keying differently.
            Self::Realise(context) => context.iter().map(|e| e.display().into_bytes()).collect(),
            // Every attribute, name and tagged value, in the map's order. The
            // tag is part of the key because `{ shallow = true; }` and
            // `{ shallow = "1"; }` are different inputs that would otherwise
            // digest alike.
            Self::FetchTree(r) => {
                let mut parts = vec![r.fetcher.as_str().as_bytes().to_vec()];
                for (name, value) in &r.attrs {
                    parts.push(name.clone().into_bytes());
                    parts.push(value.tag().as_bytes().to_vec());
                    parts.push(value.text().into_bytes());
                }
                parts
            }
            // Every attribute, name and tagged value, exactly as
            // `FetchTree`'s and for the reason given there.
            Self::FlakeRefToString(attrs) => {
                let mut parts = Vec::new();
                for (name, value) in attrs {
                    parts.push(name.clone().into_bytes());
                    parts.push(value.tag().as_bytes().to_vec());
                    parts.push(value.text().into_bytes());
                }
                parts
            }
            _ => Vec::new(),
        }
    }

    /// A dense index per variant, for tests that must cover the enum rather
    /// than a sample of it.
    ///
    /// # This is the guard, and the chain it starts is the point
    ///
    /// `CopyToStore` shipped with a tag and an `ask` arm and no decoder arm,
    /// so every witness naming one failed to parse and every evaluation
    /// containing `"${./x}"` missed the cache for ever -- silently, and
    /// without even registering as a wasted replay, because the bail happened
    /// before the replay ran. It was found by reading, and fixed by hand
    /// (ENG-12443); nothing stopped the next one.
    ///
    /// Adding a variant now walks a chain that ends in the decoder:
    ///
    /// 1. this match is exhaustive, so it does not compile until the variant
    ///    is named and given an index;
    /// 2. `every_question_variant_is_listed` requires that index to be below
    ///    [`Question::VARIANT_COUNT`], so the count has to be raised;
    /// 3. raising it makes the same test demand a sample in
    ///    [`Question::one_of_each`];
    /// 4. and the sample is fed through the codec by
    ///    `every_question_variant_round_trips_through_the_witness_codec`,
    ///    which fails until `question_from` learns the tag.
    ///
    /// A macro generating the enum would collapse that to one step, and was
    /// the first attempt here. It cannot express this enum: `FindFile` is a
    /// struct variant and `NixPath` a unit variant, and flattening them into
    /// a uniform shape to suit the guard would be the guard deciding the data
    /// model.
    #[cfg(test)]
    const fn variant_index(&self) -> usize {
        match self {
            Self::ReadFile(_) => 0,
            Self::ReadDir(_) => 1,
            Self::PathExists(_) => 2,
            Self::FileType(_) => 3,
            Self::GetEnv(_) => 4,
            Self::CopyToStore(_) => 5,
            Self::FindFile { .. } => 6,
            Self::NixPath => 7,
            Self::EnsurePath(_) => 8,
            Self::StoreText { .. } => 9,
            Self::StoreFiltered(_) => 10,
            Self::Fetch(_) => 11,
            Self::FetchTree(_) => 12,
            Self::FileTypeResolved(_) => 13,
            Self::LockFlake(_) => 14,
            Self::Realise(_) => 15,
            Self::ReadFileBytes(_) => 16,
            Self::ParseFlakeRef(_) => 17,
            Self::FlakeRefToString(_) => 18,
        }
    }

    /// How many variants [`Question::variant_index`] can return.
    #[cfg(test)]
    const VARIANT_COUNT: usize = 19;

    /// One instance of every variant, in index order.
    #[cfg(test)]
    fn one_of_each() -> Vec<Self> {
        vec![
            Self::ReadFile("/argument/read-file".to_owned()),
            Self::ReadDir("/argument/read-dir".to_owned()),
            Self::PathExists("/argument/path-exists".to_owned()),
            Self::FileType("/argument/file-type".to_owned()),
            Self::GetEnv("ARGUMENT_GET_ENV".to_owned()),
            Self::CopyToStore("/argument/copy-to-store".to_owned()),
            Self::FindFile {
                entries: vec![
                    SearchPathEntry {
                        prefix: "nixpkgs".to_owned(),
                        path: "/argument/nixpkgs".to_owned(),
                    },
                    SearchPathEntry {
                        prefix: String::new(),
                        path: "/argument/fallback".to_owned(),
                    },
                ],
                name: "nixpkgs".to_owned(),
            },
            Self::NixPath,
            Self::EnsurePath("/argument/ensure-path".to_owned()),
            Self::StoreText {
                name: "argument-store-text".to_owned(),
                contents: "argument contents".to_owned(),
                references: vec![
                    "/argument/store-text-ref-a".to_owned(),
                    "/argument/store-text-ref-b".to_owned(),
                ],
            },
            Self::StoreFiltered(Box::new(crate::task::FilteredCopy {
                root: "/argument/store-filtered".to_owned(),
                name: "argument-name".to_owned(),
                method: crate::task::PathMethod::Flat,
                accepted: Some(vec![
                    crate::task::AcceptedPath {
                        path: "/argument/store-filtered/a".to_owned(),
                        file_type: FileType::Regular,
                    },
                    crate::task::AcceptedPath {
                        path: "/argument/store-filtered/d".to_owned(),
                        file_type: FileType::Directory,
                    },
                ]),
                expected_sha256: Some(
                    "sha256-1BdlSaqjNlSVCcgD/PocqAwbnGQ+lyfL6h9WK6+MCJc=".to_owned(),
                ),
                inherit_references: true,
            })),
            Self::Fetch(Box::new(crate::task::FetchRequest {
                url: "https://argument.example/fetch.tar.gz".to_owned(),
                name: "argument-fetch-name".to_owned(),
                kind: crate::task::FetchKind::Tarball,
                expected_sha256: Some(
                    "sha256-1BdlSaqjNlSVCcgD/PocqAwbnGQ+lyfL6h9WK6+MCJc=".to_owned(),
                ),
            })),
            Self::FetchTree(Box::new(crate::task::FetchTreeRequest {
                attrs: [
                    (
                        "type".to_owned(),
                        crate::task::TreeAttr::Str("git".to_owned()),
                    ),
                    (
                        "url".to_owned(),
                        crate::task::TreeAttr::Str("/argument/repo".to_owned()),
                    ),
                    ("shallow".to_owned(), crate::task::TreeAttr::Bool(true)),
                    ("revCount".to_owned(), crate::task::TreeAttr::Int(7)),
                ]
                .into_iter()
                .collect(),
                fetcher: crate::task::TreeFetcher::Git,
            })),
            Self::FileTypeResolved("/argument/file-type-resolved".to_owned()),
            Self::LockFlake("path:/argument/flake".to_owned()),
            // All three element shapes, so the codec is exercised on the two
            // that carry a marker character as well as the bare one.
            Self::Realise(vec![
                crate::value2::ContextElem::Opaque("/argument/realise-opaque".into()),
                crate::value2::ContextElem::DrvDeep("/argument/realise-deep.drv".into()),
                crate::value2::ContextElem::Built {
                    drv: "/argument/realise-built.drv".into(),
                    output: "dev".into(),
                },
            ]),
            Self::ReadFileBytes("/argument/read-file-bytes".to_owned()),
            Self::ParseFlakeRef("github:argument/parse-flake-ref".to_owned()),
            // All three attr shapes, so the codec round-trip exercises every
            // tag the triplet encoding can carry.
            Self::FlakeRefToString(
                [
                    (
                        "type".to_owned(),
                        crate::task::TreeAttr::Str("github".to_owned()),
                    ),
                    ("shallow".to_owned(), crate::task::TreeAttr::Bool(false)),
                    ("revCount".to_owned(), crate::task::TreeAttr::Int(11)),
                ]
                .into_iter()
                .collect(),
            ),
        ]
    }

    /// Ask this question of a host and digest the answer.
    ///
    /// The digest, not the answer, is what a read set carries: a read set
    /// holding the contents of every file read would be larger than the thing
    /// it is caching.
    #[must_use]
    pub fn ask(&self, host: &dyn Host) -> Hash {
        match self {
            Self::LockFlake(flake_ref) => digest_flake_call(&host.lock_flake(flake_ref)),
            Self::ReadFile(path) => match host.read_file(path) {
                Ok(text) => digest(&[b"file-ok", text.as_bytes()]),
                Err(error) => digest(&[b"file-err", error.as_bytes()]),
            },
            Self::ReadFileBytes(path) => digest_file_bytes(&host.read_file_bytes(path)),
            Self::ReadDir(path) => match host.read_dir(path) {
                Ok(entries) => {
                    // read_dir already sorts by name, so the digest does not
                    // depend on the order the filesystem happened to return.
                    let mut parts: Vec<Vec<u8>> = vec![b"dir-ok".to_vec()];
                    for (name, kind) in entries {
                        parts.push(name.into_bytes());
                        parts.push(kind.as_str().as_bytes().to_vec());
                    }
                    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
                    digest(&refs)
                }
                Err(error) => digest(&[b"dir-err", error.as_bytes()]),
            },
            Self::PathExists(path) => {
                digest(&[b"exists", if host.path_exists(path) { b"1" } else { b"0" }])
            }
            Self::FileType(path) => digest_file_type(&host.file_type(path)),
            // A different domain prefix from `FileType`'s, so the two cannot
            // digest equal on a path where `lstat` and `stat` happen to
            // agree. They are different questions; a key must be able to say
            // which one was asked.
            Self::FileTypeResolved(path) => match host.file_type_resolved(path) {
                Ok(kind) => digest(&[b"kind-resolved-ok", kind.as_str().as_bytes()]),
                Err(error) => digest(&[b"kind-resolved-err", error.as_bytes()]),
            },
            // Unset and empty are different facts, and a read set that
            // conflated them would hit across a change between the two.
            Self::GetEnv(name) => match host.get_env(name) {
                Some(value) => digest(&[b"env-set", value.as_bytes()]),
                None => digest(&[b"env-unset"]),
            },
            Self::CopyToStore(path) => digest_store_copy(&host.copy_to_store(path)),
            Self::StoreText {
                name,
                contents,
                references,
            } => digest_store_text(&host.store_text(name, contents, references)),
            Self::FindFile { entries, name } => digest_find_file(&host.find_file(entries, name)),
            Self::NixPath => digest_nix_path(&host.nix_path()),
            Self::EnsurePath(path) => digest_store_ensure(&host.ensure_path(path)),
            Self::Realise(context) => digest_realise(&host.realise(context)),
            Self::StoreFiltered(request) => digest_store_copy(&host.store_filtered(request)),
            Self::Fetch(request) => digest_store_copy(&host.fetch(request)),
            Self::FetchTree(request) => digest_store_copy(&host.fetch_tree(request)),
            Self::ParseFlakeRef(flake_ref) => digest_store_copy(&host.parse_flake_ref(flake_ref)),
            Self::FlakeRefToString(attrs) => digest_store_copy(&host.flake_ref_to_string(attrs)),
        }
    }
}

fn digest(parts: &[&[u8]]) -> Hash {
    hash::tagged(READ_TAG, parts)
}

/// One body for the recorder and for replay, for the reason
/// [`digest_flake_call`] gives: a difference between the two is a permanent
/// miss or a false hit, never a reported mismatch.
fn digest_file_bytes(answer: &Result<Vec<u8>, String>) -> Hash {
    match answer {
        Ok(bytes) => digest(&[b"file-bytes-ok", bytes]),
        Err(error) => digest(&[b"file-bytes-err", error.as_bytes()]),
    }
}

/// The three outcomes of a store copy, kept apart. "No store here" is not the
/// same fact as "the copy failed", and neither is the same as a store path:
/// conflating them would let a witness recorded in one embedding hit in
/// another that would have answered differently.
/// The three outcomes of `store_text`, kept apart for the reason
/// [`digest_store_copy`] keeps its three apart.
fn digest_store_text(answer: &Result<String, StoreError>) -> Hash {
    match answer {
        Ok(store_path) => digest(&[b"text-ok", store_path.as_bytes()]),
        Err(StoreError::Failed(error)) => digest(&[b"text-err", error.as_bytes()]),
        Err(StoreError::Unsupported(error)) => digest(&[b"text-unsupported", error.as_bytes()]),
        Err(StoreError::NoStore) => digest(&[b"text-absent"]),
    }
}

/// Digest a lock, for the recorder and for replay.
///
/// One function and not two, because these are the two sides of the same
/// comparison: the recorder writes this digest into the read set and
/// [`Question::ask`] recomputes it on replay, so a difference between them is
/// not a mismatch that gets reported -- it is a permanent miss, or worse, a
/// hit that should not have been. Recording and replaying through one body is
/// what makes that impossible rather than merely unlikely.
///
/// The `call-flake.nix` source is deliberately outside the digest. It is a
/// compile-time constant of the embedder binary, identical for every call in
/// a process, so including it would add a field that never varies; if it ever
/// did vary the binary changed, which the evaluator settings fingerprint
/// already covers.
fn digest_flake_call(answer: &Result<crate::host::FlakeCall, StoreError>) -> Hash {
    // Reduced to the shape `digest_store_copy` takes, so the four outcomes
    // (ok, failed, unsupported, no store) are told apart by the one function
    // that already knows how rather than by a second hand-rolled match.
    let reduced: Result<String, StoreError> = match answer {
        Ok(call) => Ok(format!("{}\0{}", call.lock_file, call.overrides)),
        Err(e) => Err(e.clone()),
    };
    digest_store_copy(&reduced)
}

fn digest_store_copy(answer: &Result<String, StoreError>) -> Hash {
    match answer {
        Ok(store_path) => digest(&[b"store-ok", store_path.as_bytes()]),
        Err(StoreError::Failed(error)) => digest(&[b"store-err", error.as_bytes()]),
        // A fourth outcome, kept apart for the reason the other three are:
        // "this backend will not carry it" is not "the store could not", and
        // a witness recorded under one must not hit under the other.
        Err(StoreError::Unsupported(error)) => digest(&[b"store-unsupported", error.as_bytes()]),
        Err(StoreError::NoStore) => digest(&[b"store-absent"]),
    }
}

/// The three outcomes of `ensure_path`, kept apart for the same reason
/// [`digest_store_copy`] keeps its three apart: "no store here" is not "the
/// store could not produce it", and neither is success.
fn digest_store_ensure(answer: &Result<(), StoreError>) -> Hash {
    match answer {
        Ok(()) => digest(&[b"ensure-ok"]),
        Err(StoreError::Failed(error)) => digest(&[b"ensure-err", error.as_bytes()]),
        Err(StoreError::Unsupported(error)) => digest(&[b"ensure-unsupported", error.as_bytes()]),
        Err(StoreError::NoStore) => digest(&[b"ensure-absent"]),
    }
}

/// The four outcomes of `realise`, kept apart for the reason
/// [`digest_store_copy`] keeps its four apart, and reduced to that function
/// so the four are told apart in one place rather than two.
///
/// The rewrite map is flattened into the reduced string with separators that
/// cannot occur in a store path, so a map of one pair cannot digest equal to
/// a differently-split map of the same bytes.
fn digest_realise(answer: &Result<std::collections::BTreeMap<String, String>, StoreError>) -> Hash {
    let reduced: Result<String, StoreError> = match answer {
        Ok(rewrites) => Ok(rewrites
            .iter()
            .map(|(from, to)| format!("{from}\0{to}"))
            .collect::<Vec<_>>()
            .join("\0")),
        Err(e) => Err(e.clone()),
    };
    digest_store_copy(&reduced)
}

/// The questions one evaluation asked, in the order it asked them, with the
/// answers it received.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReadSet {
    entries: Vec<(Question, Hash)>,
}

impl ReadSet {
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The questions alone, which is what gets remembered as a witness.
    #[must_use]
    pub fn questions(&self) -> Vec<Question> {
        self.entries.iter().map(|(q, _)| q.clone()).collect()
    }

    /// `H(identity, (tag, arg, answer)*)`, order-sensitive.
    ///
    /// Order is part of the key because the question sequence is itself a
    /// function of the answers: two evaluations that asked the same questions
    /// in different orders are not the same evaluation.
    #[must_use]
    pub fn key(&self, identity: &EvalId) -> Hash {
        let mut parts: Vec<Vec<u8>> = vec![identity.as_hash().as_bytes().to_vec()];
        for (question, answer) in &self.entries {
            parts.push(vec![question.tag()]);
            parts.push(question.arg().as_bytes().to_vec());
            parts.extend(question.key_parts());
            parts.push(answer.as_bytes().to_vec());
        }
        let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
        hash::tagged(EVAL_TAG, &refs)
    }

    /// Ask a remembered question list again and build the read set from the
    /// answers given *now*. The recorded answers are deliberately not an input:
    /// computing the key from what was observed rather than what was expected
    /// is what makes a stale witness a miss instead of a wrong answer.
    ///
    /// `None` when the embedder has forbidden the evaluator to reach the world
    /// and the witness names anything at all, because replaying it would do
    /// exactly the reads `restrict-eval` and `pure-eval` exist to prevent.
    ///
    /// # Why the check is here and not only in the memo key
    ///
    /// `Question::ask` calls `host.read_file` and friends directly. The
    /// evaluator's own access check lives in `eval::answer_path`, which replay
    /// never enters, so this path had no check at all (ENG-12543). Before the
    /// settings went into the memo key, that was live and not theoretical: a
    /// cache filled with reads allowed, then looked up under `pure-eval`, read
    /// the file again *and* served the answer, measured at 8065be845 as
    /// `status=ok value="secret" memo_hit=true reads=["/etc/shadow"]`.
    ///
    /// Folding the purity settings into the identity (ENG-12541) closed the
    /// exploit incidentally: the two settings now address different rows, so
    /// the pure-eval lookup finds no witness and never gets here. That is a
    /// real fix and a fragile place to leave the guarantee, because it holds
    /// only as long as those fields stay in the key -- a property of a
    /// different file, provable only by reading both. This check makes it
    /// local: no read happens here whatever the key does.
    ///
    /// The test is [`crate::purity::verdict`], question by question, and only
    /// `Verdict::Ask` replays. That is the same rule `eval::answer_path`
    /// applies to a live evaluation, read through the same table, so replay
    /// cannot permit a question the evaluator would have refused nor refuse
    /// one it would have served.
    ///
    /// Both arguments to that table are read here, not just the settings.
    /// [`crate::purity::PathReads`] says whether a plain filesystem read goes
    /// through the embedder's accessor, and it decides five of the rows
    /// (ENG-12792), so a witness recorded by the `nix` binary -- where it
    /// does -- must not be replayed by a standalone embedding under a purity
    /// setting, where the same reads would come from `std::fs` and honour
    /// nothing. Reading the table rather than the settings is what makes that
    /// automatic: the rows move and this check moves with them.
    ///
    /// The three non-`Ask` verdicts all block, for three different reasons.
    /// `Refuse` is the one this check exists for. `EmptyString` blocks because
    /// the recorded answer for `getEnv` was the environment's value and the
    /// live answer under either setting is `""`, so replaying would hand back
    /// the impure one. `Error` blocks because a live evaluation would have
    /// failed rather than asked, and a witness recorded before the setting was
    /// on has an answer for a question that now must not be asked at all.
    ///
    /// Being *coarser* than the table was the first attempt and it was wrong
    /// in the direction that looks safe: refusing every witness naming any
    /// question under either setting made `builtins.toFile` and a path
    /// interpolation -- both of which pure eval permits -- record a result
    /// that could never be found again, which
    /// `maintainers/ix/cache-semantics-gate.sh` reports as an unreachable memo
    /// key rather than tolerating.
    ///
    /// An empty witness still replays, and should. It names no reads, so there
    /// is nothing to forbid, and a pure expression keeps its cache under
    /// `pure-eval` -- which is the one case where caching is unambiguously
    /// safe.
    #[must_use]
    pub fn replay(
        questions: &[Question],
        host: &dyn Host,
        settings: &crate::eval::Settings,
    ) -> Option<Self> {
        let purity = settings.purity();
        let reads = settings.path_reads;
        if purity.any()
            && questions.iter().any(|question| {
                !matches!(
                    crate::purity::verdict(&question.as_need_path(), purity, reads),
                    crate::purity::Verdict::Ask
                )
            })
        {
            return None;
        }
        Some(Self {
            entries: questions
                .iter()
                .map(|question| (question.clone(), question.ask(host)))
                .collect(),
        })
    }
}

/// A line the evaluation printed rather than returned.
///
/// `builtins.trace` and cppnix's `warn()` both go out through [`Host`], and
/// both are part of what an evaluation produced. One ordered list holds them
/// together rather than two lists side by side, because cppnix emits them
/// interleaved in the order they happen and two lists could only be replayed
/// in an order nothing produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Emission {
    Warn(String),
    Trace(String),
}

impl Emission {
    /// Say it again, through the host a served evaluation was given.
    pub fn replay(&self, host: &dyn Host) {
        match self {
            Self::Warn(message) => host.warn(message),
            Self::Trace(message) => host.trace(message),
        }
    }

    /// `(kind, text)`, for the codec.
    const fn parts(&self) -> (&'static str, &String) {
        match self {
            Self::Warn(message) => ("warn", message),
            Self::Trace(message) => ("trace", message),
        }
    }

    /// The inverse of [`Emission::parts`]. A kind this build does not know
    /// makes the whole row undecodable rather than a line silently dropped:
    /// the point of storing these is that a served run says the same thing,
    /// and half of what it said is not that.
    fn from_parts(kind: &str, message: String) -> Option<Self> {
        match kind {
            "warn" => Some(Self::Warn(message)),
            "trace" => Some(Self::Trace(message)),
            _ => None,
        }
    }
}

/// A host that answers through another host and remembers what it was asked.
///
/// `RefCell` rather than `&mut self` because [`Host`] takes `&self`: the
/// evaluator holds it immutably while the scheduler drives it, and recording
/// is the only mutation.
pub struct RecordingHost<H> {
    inner: H,
    /// Whether `warn` and `trace` reach `inner` as well as being recorded.
    ///
    /// Off for a sampled verification, which re-does work the memo already
    /// answered: the served answer's emissions are replayed instead, so a
    /// reader must not also see this run's copy. It is one field rather than
    /// a wrapper host because a wrapper has to forward every other method by
    /// hand, and the one that got written forgot `store_text` and answered
    /// "no store behind this evaluator" on behalf of a host that had one.
    /// [`Host`] has no default method bodies precisely so that mistake cannot
    /// be silent; not needing the wrapper at all is better still.
    forward_emissions: bool,
    log: RefCell<Vec<(Question, Hash)>>,
    /// Warnings and traces the evaluation emitted, interleaved in the order
    /// it emitted them.
    ///
    /// Not questions: an emission is an output, so it has no answer to key
    /// on. It still has to be remembered, because a memoised result served on
    /// a later run reproduces the value and would otherwise stay silent about
    /// something the first run said out loud -- which is `eval-cache-dir`
    /// changing what the evaluator tells the reader.
    emissions: RefCell<Vec<Emission>>,
    /// Slow questions begun through [`Host::begin`] and not yet collected.
    ///
    /// A question begun in the background is not recorded when it is asked,
    /// because the answer -- which is half of what the read set stores -- does
    /// not exist yet. So the question is held here and noted when its answer
    /// arrives, which puts it in the log at the point the evaluation actually
    /// received it.
    ///
    /// That the log stays deterministic despite this is a property of the
    /// scheduler and not of this map. One evaluation can have several
    /// questions outstanding since ENG-13150, so this map does hold more
    /// than one entry at a time -- but `crate::eval::drive_concurrent`
    /// collects strictly oldest-token-first, and tokens are minted at ask,
    /// so begun questions land in the log in ask order however their
    /// answers raced. A begun question CAN land after a synchronous one
    /// asked later by a sibling strand, which reorders the log relative to
    /// a host that begins nothing; that moves `ReadSet::key` the same way
    /// on every run against the same host, a deterministic miss and not a
    /// wrong answer. `crate::vm::Fiber`'s doc is where the invariant that
    /// keeps all of this sound is written down.
    begun: RefCell<std::collections::HashMap<u64, Question>>,
}

impl<H: Host> RecordingHost<H> {
    #[must_use]
    pub fn new(inner: H) -> Self {
        Self {
            inner,
            forward_emissions: true,
            log: RefCell::new(Vec::new()),
            emissions: RefCell::new(Vec::new()),
            begun: RefCell::new(std::collections::HashMap::new()),
        }
    }

    /// A recorder that answers every question and repeats nothing.
    ///
    /// Only the two outputs are dropped. Every read still goes through,
    /// because the point of a verification run is to evaluate against the
    /// same world the cached answer was taken from. See
    /// [`RecordingHost::forward_emissions`].
    #[must_use]
    pub fn quiet(inner: H) -> Self {
        Self {
            forward_emissions: false,
            ..Self::new(inner)
        }
    }

    /// Everything asked since the last [`take`].
    ///
    /// [`take`]: RecordingHost::take
    #[must_use]
    pub fn take(&self) -> ReadSet {
        ReadSet {
            entries: core::mem::take(&mut self.log.borrow_mut()),
        }
    }

    /// Everything warned since the last call.
    #[must_use]
    pub fn take_emissions(&self) -> Vec<Emission> {
        core::mem::take(&mut self.emissions.borrow_mut())
    }

    /// Record a question and its answer. The answer is digested from the value
    /// actually returned, so the log cannot disagree with what the evaluator
    /// was told.
    fn note(&self, question: Question, answer: Hash) {
        self.log.borrow_mut().push((question, answer));
    }
}

/// The [`Question`] a slow question records as.
///
/// The same question the blocking method would have recorded, which is the
/// point: a read set must not be able to say which of the two routes to the
/// host an evaluation happened to take. A witness recorded through `begin`
/// replays through `Question::ask`, which calls the blocking method.
fn slow_question(question: &crate::host::Slow<'_>) -> Question {
    match question {
        crate::host::Slow::Fetch(request) => Question::Fetch(Box::new((*request).clone())),
        crate::host::Slow::FetchTree(request) => Question::FetchTree(Box::new((*request).clone())),
        crate::host::Slow::Flake(flake_ref) => Question::LockFlake((*flake_ref).to_owned()),
        crate::host::Slow::Realise(context) => Question::Realise((*context).to_vec()),
    }
}

impl<H: Host> Host for RecordingHost<H> {
    fn read_file(&self, path: &str) -> Result<String, String> {
        let answer = self.inner.read_file(path);
        let digest = match &answer {
            Ok(text) => digest(&[b"file-ok", text.as_bytes()]),
            Err(error) => digest(&[b"file-err", error.as_bytes()]),
        };
        self.note(Question::ReadFile(path.to_owned()), digest);
        answer
    }

    fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
        let answer = self.inner.read_file_bytes(path);
        self.note(
            Question::ReadFileBytes(path.to_owned()),
            digest_file_bytes(&answer),
        );
        answer
    }

    fn read_dir(&self, path: &str) -> Result<Vec<(String, FileType)>, String> {
        let answer = self.inner.read_dir(path);
        let digest = match &answer {
            Ok(entries) => {
                let mut parts: Vec<Vec<u8>> = vec![b"dir-ok".to_vec()];
                for (name, kind) in entries {
                    parts.push(name.clone().into_bytes());
                    parts.push(kind.as_str().as_bytes().to_vec());
                }
                let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
                super::readset::digest(&refs)
            }
            Err(error) => digest(&[b"dir-err", error.as_bytes()]),
        };
        self.note(Question::ReadDir(path.to_owned()), digest);
        answer
    }

    fn path_exists(&self, path: &str) -> bool {
        let answer = self.inner.path_exists(path);
        self.note(
            Question::PathExists(path.to_owned()),
            digest(&[b"exists", if answer { b"1" } else { b"0" }]),
        );
        answer
    }

    fn copy_to_store(&self, path: &str) -> Result<String, StoreError> {
        let answer = self.inner.copy_to_store(path);
        self.note(
            Question::CopyToStore(path.to_owned()),
            digest_store_copy(&answer),
        );
        answer
    }

    fn store_text(
        &self,
        name: &str,
        contents: &str,
        references: &[String],
    ) -> Result<String, StoreError> {
        let answer = self.inner.store_text(name, contents, references);
        self.note(
            Question::StoreText {
                name: name.to_owned(),
                contents: contents.to_owned(),
                references: references.to_vec(),
            },
            digest_store_text(&answer),
        );
        answer
    }

    fn ensure_path(&self, path: &str) -> Result<(), StoreError> {
        let answer = self.inner.ensure_path(path);
        self.note(
            Question::EnsurePath(path.to_owned()),
            digest_store_ensure(&answer),
        );
        answer
    }

    fn realise(
        &self,
        context: &[crate::value2::ContextElem],
    ) -> Result<std::collections::BTreeMap<String, String>, StoreError> {
        let answer = self.inner.realise(context);
        self.note(Question::Realise(context.to_vec()), digest_realise(&answer));
        answer
    }

    /// Recorded as the [`Question::StoreText`] it is.
    ///
    /// Not a variant of its own, because a witness exists to be re-asked and
    /// re-asking this is re-asking that: `writeDerivation` is
    /// `addTextToStore` of the same three arguments, so a replay that calls
    /// `store_text` with them performs the identical store operation. A
    /// separate question would have needed a wire tag and a codec to say
    /// nothing new.
    ///
    /// The name is recorded with its `.drv` suffix, the way the store sees
    /// it, so it cannot collide with a `builtins.toFile "x"` of the same
    /// bytes under the bare name.
    fn write_derivation(
        &self,
        name: &str,
        aterm: &str,
        references: &[String],
    ) -> Result<String, StoreError> {
        let answer = self.inner.write_derivation(name, aterm, references);
        self.note(
            Question::StoreText {
                name: format!("{name}.drv"),
                contents: aterm.to_owned(),
                references: references.to_vec(),
            },
            digest_store_text(&answer),
        );
        answer
    }

    fn store_filtered(&self, request: &crate::task::FilteredCopy) -> Result<String, StoreError> {
        let answer = self.inner.store_filtered(request);
        self.note(
            Question::StoreFiltered(Box::new(request.clone())),
            digest_store_copy(&answer),
        );
        answer
    }

    fn fetch(&self, request: &crate::task::FetchRequest) -> Result<String, StoreError> {
        let answer = self.inner.fetch(request);
        self.note(
            Question::Fetch(Box::new(request.clone())),
            digest_store_copy(&answer),
        );
        answer
    }

    fn fetch_tree(&self, request: &crate::task::FetchTreeRequest) -> Result<String, StoreError> {
        let answer = self.inner.fetch_tree(request);
        self.note(
            Question::FetchTree(Box::new(request.clone())),
            digest_store_copy(&answer),
        );
        answer
    }

    fn lock_flake(&self, flake_ref: &str) -> Result<crate::host::FlakeCall, StoreError> {
        let answer = self.inner.lock_flake(flake_ref);
        // The lock file and the overrides go into the digest; the
        // `call-flake.nix` source does not. It is a compile-time constant of
        // the embedder binary, identical for every call in a process, so
        // digesting it would add a field that never varies -- and if it ever
        // did vary, the binary changed, which the evaluator settings
        // fingerprint already covers.
        self.note(
            Question::LockFlake(flake_ref.to_owned()),
            digest_flake_call(&answer),
        );
        answer
    }

    fn parse_flake_ref(&self, flake_ref: &str) -> Result<String, StoreError> {
        let answer = self.inner.parse_flake_ref(flake_ref);
        self.note(
            Question::ParseFlakeRef(flake_ref.to_owned()),
            digest_store_copy(&answer),
        );
        answer
    }

    fn flake_ref_to_string(
        &self,
        attrs: &std::collections::BTreeMap<String, crate::task::TreeAttr>,
    ) -> Result<String, StoreError> {
        let answer = self.inner.flake_ref_to_string(attrs);
        self.note(
            Question::FlakeRefToString(attrs.clone()),
            digest_store_copy(&answer),
        );
        answer
    }

    /// Forwarded *and* remembered, and the same for [`Host::trace`].
    ///
    /// Forwarded because the embedder's logger is where the line belongs on
    /// the run that produced it. Remembered because a run served from the
    /// memo table never executes the code that emitted it, and a cached run
    /// that stayed quiet would tell its reader less than the run that filled
    /// the cache did -- `eval-cache-dir` deciding how much the evaluator
    /// says, which is the same class of divergence as deciding what it
    /// answers.
    fn warn(&self, message: &str) {
        self.emissions
            .borrow_mut()
            .push(Emission::Warn(message.to_owned()));
        if self.forward_emissions {
            self.inner.warn(message);
        }
    }

    fn trace(&self, message: &str) {
        self.emissions
            .borrow_mut()
            .push(Emission::Trace(message.to_owned()));
        if self.forward_emissions {
            self.inner.trace(message);
        }
    }

    fn file_type(&self, path: &str) -> Result<Option<FileType>, String> {
        let answer = self.inner.file_type(path);
        let digest = digest_file_type(&answer);
        self.note(Question::FileType(path.to_owned()), digest);
        answer
    }

    /// Recorded separately from [`Host::file_type`] because it is a separate
    /// question with a separate answer. `Host::resolve_import` keeps its
    /// default body here, as its doc says it must, and that body now asks
    /// this -- so the import's world-read lands in the log through this
    /// method rather than through `file_type`.
    fn file_type_resolved(&self, path: &str) -> Result<FileType, String> {
        let answer = self.inner.file_type_resolved(path);
        let digest = match &answer {
            Ok(kind) => digest(&[b"kind-resolved-ok", kind.as_str().as_bytes()]),
            Err(error) => digest(&[b"kind-resolved-err", error.as_bytes()]),
        };
        self.note(Question::FileTypeResolved(path.to_owned()), digest);
        answer
    }

    fn find_file(&self, entries: &[SearchPathEntry], name: &str) -> Result<String, LookupError> {
        let answer = self.inner.find_file(entries, name);
        self.note(
            Question::FindFile {
                entries: entries.to_vec(),
                name: name.to_owned(),
            },
            digest_find_file(&answer),
        );
        answer
    }

    fn nix_path(&self) -> Result<Vec<SearchPathEntry>, LookupError> {
        let answer = self.inner.nix_path();
        self.note(Question::NixPath, digest_nix_path(&answer));
        answer
    }

    fn get_env(&self, name: &str) -> Option<String> {
        let answer = self.inner.get_env(name);
        let digest = match &answer {
            Some(value) => digest(&[b"env-set", value.as_bytes()]),
            None => digest(&[b"env-unset"]),
        };
        self.note(Question::GetEnv(name.to_owned()), digest);
        answer
    }

    /// Both halves are forwarded, and the recording happens on the collecting
    /// half.
    ///
    /// A wrapper that inherited the defaults here would be worse than one
    /// that forgot an effect: `begin` returning `None` does not break
    /// anything, it only turns the asynchronous path off silently, and a
    /// performance feature that quietly does not apply is the kind of thing
    /// that stays broken for a year. `the_recorder_forwards_a_begun_question`
    /// is what says it does not.
    fn begin(&self, question: &crate::host::Slow<'_>) -> Option<crate::host::Ticket> {
        let ticket = self.inner.begin(question)?;
        self.begun
            .borrow_mut()
            .insert(ticket.0, slow_question(question));
        Some(ticket)
    }

    fn collect(&self, ticket: crate::host::Ticket, block: bool) -> Option<crate::host::SlowAnswer> {
        let answer = self.inner.collect(ticket, block)?;
        // Only now is there an answer to digest, so only now can the question
        // be recorded. A `begin` this recorder did not see -- which cannot
        // happen, since it is the only door -- leaves nothing to note rather
        // than noting a question with a made-up answer.
        if let Some(question) = self.begun.borrow_mut().remove(&ticket.0) {
            let digest = match &answer {
                crate::host::SlowAnswer::Store(answer) => digest_store_copy(answer),
                crate::host::SlowAnswer::Flake(answer) => digest_flake_call(answer),
                crate::host::SlowAnswer::Realise(answer) => digest_realise(answer),
            };
            self.note(question, digest);
        }
        Some(answer)
    }
}

// -------------------------------------------------------------- result cache

use ix_kernel::canon::{self, CanonValue};
use ix_kernel::cas::Cas;
use ix_kernel::dispatch::{PerformCtx, on_perform};
use ix_kernel::rows::{DirRows, Lookup};
use ix_kernel::{Domain, EffectLock, KernelConfig, KernelError, MemoTable, Policy};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The effect being memoised: evaluating one module to a printed result.
#[must_use]
pub fn eval_domain() -> Domain {
    Domain::mint("ix-eval.evaluate", "module-result")
}

/// A printed evaluation outcome: everything the server would have said.
///
/// `emissions` is part of the outcome and not a bystander. cppnix warns about
/// six derivation attributes `__structuredAttrs` quietly disables, and
/// `builtins.trace` prints on demand; an evaluation served from the memo
/// table has to say both again, or the reader is told less than they would
/// have been without `eval-cache-dir` -- the setting changing what the
/// evaluator says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EvalResult {
    pub status: String,
    pub value: String,
    pub emissions: Vec<Emission>,
    /// Which kind of refusal `status` records, when it records one.
    ///
    /// A field beside the status rather than something encoded into it. The
    /// stored form is a canonical map keyed by name and `emissions` above
    /// already showed that a reader tolerates a key it does not find, so
    /// there is no reason to smuggle a second fact through the status string
    /// and parse it back out.
    pub token: Option<crate::refusal::RefusalToken>,
    /// Where the failure happened, when it happened somewhere.
    ///
    /// Part of the outcome for the same reason `token` is: the embedder
    /// renders `at /path/file.nix:LINE:COL` from it and prints the source
    /// line underneath, so a served answer that dropped it would say less on
    /// the second run than on the first. Absent from a row written before
    /// positions existed, which reads back as `None` -- an error with no
    /// position, which is what those runs printed.
    pub pos: Option<crate::vm::SrcPos>,
}

/// What a memoised result is filed under: the module, every process setting
/// that can change what evaluating it produces, and the question that was
/// asked of it.
///
/// A newtype rather than a bare [`Hash`] so a module digest cannot be passed
/// where an identity is wanted. That was the bug: the key was the module
/// alone, so one `eval-cache-dir` shared between two store directories served
/// the first store's `outPath` to the second, and an `outPath` that is wrong
/// in all 32 characters looks exactly like a right one (ENG-12541).
///
/// The question joined it for ENG-12830. While the key was `(module,
/// settings)` only one caller could use the table at all -- the one that
/// always asks the same question, "render the whole expression" -- so `nix
/// eval` and `nix build` wrote module objects into `eval-cache-dir` and read
/// nothing back for the life of the setting. Two callers can share a module
/// and want entirely different bytes out of it, and a key that cannot say
/// which would serve one of them the other's answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EvalId {
    /// `H(module, settings, question)`, which is what everything is filed
    /// under.
    key: Hash,
    /// The module half, kept because a witness has to record which module it
    /// belongs to for the store's sweep to judge it. See
    /// [`DirWitness::put`].
    module: Hash,
}

/// Domain separation for an evaluation identity. Bumped from `-v1` when the
/// question joined the key (ENG-12830) and from `-v2` when the applied
/// arguments did (ENG-12915): rows filed under an older scheme address a
/// strictly smaller input set than their key claims, so they are retired
/// rather than reinterpreted.
const EVAL_ID_TAG: &str = "ixe-eval-identity-v3";

impl EvalId {
    /// File an evaluation under its module, its settings, what the module was
    /// applied to, and the question asked of the result.
    ///
    /// All four are required rather than defaulted, because each of the three
    /// bugs here came from a key that left one out and every such key looks
    /// exactly like a working one from the outside: it hits when the omitted
    /// field happens to match and serves the wrong answer when it does not.
    ///
    /// # What a hit is claiming
    ///
    /// That the same module, applied to the same values, under the same
    /// process settings, asked the same question, and with every world read
    /// the last run performed still giving the answer it gave then. The first
    /// four are these parameters; the fifth is the witness replay in
    /// [`ResultCache::lookup`]. The replay is not a substitute for the key --
    /// a witness is filed under the identity, so two evaluations sharing an
    /// identity share a witness, and the second one replays the first one's
    /// questions and is served the first one's answer. That was live for the
    /// flake path until the argument axis existed (ENG-12915).
    #[must_use]
    pub fn of(
        module: &Hash,
        settings: &crate::eval::Settings,
        arguments: &crate::session::Arguments,
        question: &crate::session::Question,
    ) -> Self {
        let settings = settings.fingerprint();
        let arguments = arguments.fingerprint();
        let question = question.fingerprint();
        Self {
            key: hash::tagged(
                EVAL_ID_TAG,
                &[
                    module.as_bytes(),
                    settings.as_bytes(),
                    arguments.as_bytes(),
                    question.as_bytes(),
                ],
            ),
            module: *module,
        }
    }

    #[must_use]
    pub fn as_hash(&self) -> &Hash {
        &self.key
    }

    /// The module this identity was built from.
    #[must_use]
    pub fn module(&self) -> &Hash {
        &self.module
    }
}

// ------------------------------------------------------------ witness store

/// Remembered question lists, on disk, so a cold process has something to
/// replay. Filed under an [`EvalId`], so a witness recorded under one set of
/// evaluator settings is not replayed under another.
///
/// Without this a new process holds results it can never look up: the key is
/// built from answers to last time's questions, and last time's questions died
/// with the process that asked them.
///
/// # This store needs no integrity guarantee, and that is not an oversight
///
/// Every other persisted thing here is checked. A witness is not, because a
/// wrong one cannot produce a wrong answer. The key is computed from the
/// answers observed when the questions are replayed, so a witness that names
/// the wrong questions yields a key nothing was stored under, which is a miss.
/// A witness that will not parse is also a miss. The worst a corrupted witness
/// can do is waste the reads it names, and `wasted_replays` counts that.
pub struct DirWitness {
    root: PathBuf,
}

impl DirWitness {
    pub fn open(root: impl Into<PathBuf>) -> std::io::Result<Self> {
        let root = root.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path(&self, identity: &EvalId) -> PathBuf {
        self.root.join(identity.as_hash().to_hex())
    }

    /// The questions recorded under an identity, or `None` if there are none
    /// or they do not parse. Both are misses and neither is an error.
    #[must_use]
    pub fn get(&self, identity: &EvalId) -> Option<Vec<Question>> {
        let bytes = std::fs::read(self.path(identity)).ok()?;
        witness_questions(&bytes)
    }

    /// Write a witness, recording the module it belongs to alongside the
    /// questions.
    ///
    /// # The module field is what stops the store from deleting this
    ///
    /// `Store::sweep` reclaims dead witnesses, and it used to decide which
    /// were dead by reading the *filename* and looking for an object of that
    /// name -- a rule that only worked while witnesses happened to be named
    /// by their module's object address. Renaming them to the evaluation
    /// identity (ENG-12541) broke that silently: no object is ever named
    /// after an identity, so every sweep deleted every witness, and a capped
    /// store served nothing while looking healthy. Arm E of
    /// `rust-incremental-gate.sh` went from 10 hits of 11 to 0 (ENG-12601).
    ///
    /// So the witness says which module it belongs to, in its own bytes, and
    /// the sweep reads it. A filename is now just a filename.
    pub fn put(&self, identity: &EvalId, questions: &[Question]) -> std::io::Result<()> {
        let value = CanonValue::map([
            (
                "module",
                CanonValue::Bytes(identity.module().as_bytes().to_vec()),
            ),
            (
                "questions",
                CanonValue::Array(questions.iter().map(question_value).collect()),
            ),
        ]);
        let bytes = canon::encode(&value).map_err(std::io::Error::other)?;
        // Rename into place so a reader never sees half a list.
        let temp = self.root.join(format!(
            ".tmp-{}-{}",
            std::process::id(),
            WITNESS_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&temp, &bytes)?;
        std::fs::rename(&temp, self.path(identity))
    }
}

/// The questions a witness names, or `None` if it does not parse. Both are
/// misses and neither is an error.
#[must_use]
pub fn witness_questions(bytes: &[u8]) -> Option<Vec<Question>> {
    let CanonValue::Map(entries) = canon::decode(bytes).ok()? else {
        return None;
    };
    let items = entries.iter().find_map(|(k, v)| match (k, v) {
        (CanonValue::Str(k), CanonValue::Array(items)) if k == "questions" => Some(items),
        _ => None,
    })?;
    items.iter().map(question_from).collect()
}

/// The module a witness belongs to, for [`crate::store::Store::sweep`] to
/// decide whether it is still reachable.
///
/// `None` for a witness this build cannot read, which the sweep treats as
/// dead: an unparseable witness can never produce a hit, so keeping it costs
/// bytes and buys nothing.
#[must_use]
pub fn witness_module(bytes: &[u8]) -> Option<Hash> {
    let CanonValue::Map(entries) = canon::decode(bytes).ok()? else {
        return None;
    };
    let module = entries.iter().find_map(|(k, v)| match (k, v) {
        (CanonValue::Str(k), CanonValue::Bytes(raw)) if k == "module" => Some(raw),
        _ => None,
    })?;
    Some(Hash::from_bytes(module.as_slice().try_into().ok()?))
}

static WITNESS_COUNTER: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

fn question_value(question: &Question) -> CanonValue {
    let mut parts = vec![
        CanonValue::int(question.tag()),
        CanonValue::str(question.arg()),
    ];
    // A search path lookup needs its entries to be replayable at all: the
    // name alone would be replayed against a different list and answered
    // differently, and the point of a witness is that replaying it asks what
    // the evaluation asked.
    if let Question::FindFile { entries, .. } = question {
        for e in entries {
            parts.push(CanonValue::str(&e.prefix));
            parts.push(CanonValue::str(&e.path));
        }
    }
    // Contents first, then the references, so the decoder can take the head
    // and treat the rest uniformly.
    if let Question::StoreText {
        contents,
        references,
        ..
    } = question
    {
        parts.push(CanonValue::str(contents));
        for r in references {
            parts.push(CanonValue::str(r));
        }
    }
    // A flat tail of rendered elements. Flat rather than nested because there
    // is exactly one variable-length field and it is last, so the decoder can
    // take everything after the argument without counting.
    if let Question::Realise(context) = question {
        for e in context {
            parts.push(CanonValue::str(e.display()));
        }
    }
    // Three nested arrays rather than a flat tail, because a flat one cannot
    // say where the accepted list starts without the decoder counting
    // fields -- and a filtered copy has a variable number of them.
    // Triples in a nested array, so the decoder does not have to count
    // fields to find where they start.
    if let Question::FetchTree(r) = question {
        let mut items = vec![CanonValue::str(r.fetcher.as_str())];
        for (name, value) in &r.attrs {
            items.push(CanonValue::str(name));
            items.push(CanonValue::str(value.tag()));
            items.push(CanonValue::str(value.text()));
        }
        parts.push(CanonValue::array(items));
    }
    // Triples in a nested array, exactly as the tree fetch above minus its
    // fetcher field, and for the same reason.
    if let Question::FlakeRefToString(attrs) = question {
        let mut items = Vec::new();
        for (name, value) in attrs {
            items.push(CanonValue::str(name));
            items.push(CanonValue::str(value.tag()));
            items.push(CanonValue::str(value.text()));
        }
        parts.push(CanonValue::array(items));
    }
    // One nested array rather than three flat fields, for the reason the
    // filtered copy below uses nested arrays: an optional field in a flat
    // tail cannot be told from a missing one.
    if let Question::Fetch(r) = question {
        parts.push(CanonValue::array(vec![
            CanonValue::str(&r.name),
            CanonValue::str(r.kind.as_str()),
        ]));
        parts.push(CanonValue::array(match &r.expected_sha256 {
            Some(h) => vec![CanonValue::str(h)],
            None => Vec::new(),
        }));
    }
    if let Question::StoreFiltered(r) = question {
        let mut head = vec![CanonValue::str(&r.name), CanonValue::str(r.method.as_str())];
        // Appended only when set, for the reason `key_parts` omits it: a
        // witness written before this field existed has a two-element head,
        // and the decoder below reads its absence as false.
        if r.inherit_references {
            head.push(CanonValue::str("inherit-references"));
        }
        parts.push(CanonValue::array(head));
        parts.push(CanonValue::array(match &r.expected_sha256 {
            Some(h) => vec![CanonValue::str(h)],
            None => Vec::new(),
        }));
        parts.push(CanonValue::array(match &r.accepted {
            None => vec![CanonValue::str("unfiltered")],
            Some(list) => {
                let mut items = vec![CanonValue::str("filtered")];
                for e in list {
                    items.push(CanonValue::str(&e.path));
                    items.push(CanonValue::str(e.file_type.as_str()));
                }
                items
            }
        }));
    }
    CanonValue::array(parts)
}

/// The inverse of [`question_value`].
///
/// The tag-to-variant table lives in the `questions!` macro beside the
/// variant list, so this cannot fall behind the encoder the way the
/// hand-written version did. `None` is a miss: a witness written by a build
/// that knew a question this one does not is not something to guess at.
fn question_from(value: &CanonValue) -> Option<Question> {
    let CanonValue::Array(parts) = value else {
        return None;
    };
    let (CanonValue::Int(tag), CanonValue::Str(argument)) = (parts.first()?, parts.get(1)?) else {
        return None;
    };
    let arg = argument.clone();
    match tag {
        1 => Some(Question::ReadFile(arg)),
        2 => Some(Question::ReadDir(arg)),
        3 => Some(Question::PathExists(arg)),
        4 => Some(Question::FileType(arg)),
        5 => Some(Question::GetEnv(arg)),
        // 6 was missing until ENG-12443 went through this function, which
        // cost nothing visible and cost every witness containing a store copy
        // a silent parse failure, hence a miss it looked like a cold cache
        // for.
        6 => Some(Question::CopyToStore(arg)),
        7 => {
            // Pairs, so an odd tail is a corrupt witness rather than an entry
            // with an empty path invented to fill the gap.
            let rest = parts.get(2..).unwrap_or_default();
            if rest.len() % 2 != 0 {
                return None;
            }
            let mut entries = Vec::with_capacity(rest.len() / 2);
            for pair in rest.chunks_exact(2) {
                let (CanonValue::Str(prefix), CanonValue::Str(path)) =
                    (pair.first()?, pair.get(1)?)
                else {
                    return None;
                };
                entries.push(SearchPathEntry {
                    prefix: prefix.clone(),
                    path: path.clone(),
                });
            }
            Some(Question::FindFile { entries, name: arg })
        }
        8 => Some(Question::NixPath),
        9 => Some(Question::EnsurePath(arg)),
        17 => Some(Question::ReadFileBytes(arg)),
        10 => {
            let rest = parts.get(2..).unwrap_or_default();
            let (CanonValue::Str(contents), refs) = (rest.first()?, rest.get(1..)?) else {
                return None;
            };
            let mut references = Vec::with_capacity(refs.len());
            for r in refs {
                let CanonValue::Str(r) = r else {
                    return None;
                };
                references.push(r.clone());
            }
            Some(Question::StoreText {
                name: arg,
                contents: contents.clone(),
                references,
            })
        }
        11 => question_filtered(arg, parts.get(2..).unwrap_or_default()),
        12 => question_fetch(arg, parts.get(2..).unwrap_or_default()),
        13 => question_fetch_tree(parts.get(2..).unwrap_or_default()),
        14 => Some(Question::FileTypeResolved(arg)),
        15 => Some(Question::LockFlake(arg)),
        18 => Some(Question::ParseFlakeRef(arg)),
        19 => question_flake_ref_to_string(parts.get(2..).unwrap_or_default()),
        16 => {
            let mut context = Vec::new();
            for item in parts.get(2..).unwrap_or_default() {
                let CanonValue::Str(rendered) = item else {
                    return None;
                };
                // `None` rather than a guess: an element this build cannot
                // parse would replay as a *different* question, which is the
                // one outcome worse than a miss.
                context.push(crate::value2::ContextElem::parse(rendered)?);
            }
            Some(Question::Realise(context))
        }
        _ => None,
    }
}

/// The tail of a tag-13 witness entry: one array of a fetcher name and then
/// (name, tag, value) triples. `None` for anything malformed, for the reason
/// [`question_filtered`] gives.
fn question_fetch_tree(rest: &[CanonValue]) -> Option<Question> {
    let CanonValue::Array(items) = rest.first()? else {
        return None;
    };
    let CanonValue::Str(fetcher) = items.first()? else {
        return None;
    };
    let tail = items.get(1..)?;
    if tail.len() % 3 != 0 {
        return None;
    }
    let mut attrs = std::collections::BTreeMap::new();
    for triple in tail.chunks_exact(3) {
        let (CanonValue::Str(name), CanonValue::Str(tag), CanonValue::Str(text)) =
            (triple.first()?, triple.get(1)?, triple.get(2)?)
        else {
            return None;
        };
        attrs.insert(name.clone(), crate::task::TreeAttr::parse(tag, text)?);
    }
    Some(Question::FetchTree(Box::new(
        crate::task::FetchTreeRequest {
            attrs,
            fetcher: crate::task::TreeFetcher::parse(fetcher)?,
        },
    )))
}

/// The tail of a tag-19 witness entry: one array of (name, tag, value)
/// triples, [`question_fetch_tree`]'s tail without its fetcher head. `None`
/// for anything malformed, for the reason [`question_filtered`] gives.
fn question_flake_ref_to_string(rest: &[CanonValue]) -> Option<Question> {
    let CanonValue::Array(items) = rest.first()? else {
        return None;
    };
    if items.len() % 3 != 0 {
        return None;
    }
    let mut attrs = std::collections::BTreeMap::new();
    for triple in items.chunks_exact(3) {
        let (CanonValue::Str(name), CanonValue::Str(tag), CanonValue::Str(text)) =
            (triple.first()?, triple.get(1)?, triple.get(2)?)
        else {
            return None;
        };
        attrs.insert(name.clone(), crate::task::TreeAttr::parse(tag, text)?);
    }
    Some(Question::FlakeRefToString(attrs))
}

/// The tail of a tag-12 witness entry: `[name, kind]` and `[sha256?]`.
/// `None` for anything malformed, for the reason [`question_filtered`] gives.
fn question_fetch(url: String, rest: &[CanonValue]) -> Option<Question> {
    let (CanonValue::Array(head), CanonValue::Array(sha)) = (rest.first()?, rest.get(1)?) else {
        return None;
    };
    let (CanonValue::Str(name), CanonValue::Str(kind)) = (head.first()?, head.get(1)?) else {
        return None;
    };
    let expected_sha256 = match sha.first() {
        None => None,
        Some(CanonValue::Str(h)) => Some(h.clone()),
        Some(_) => return None,
    };
    Some(Question::Fetch(Box::new(crate::task::FetchRequest {
        url,
        name: name.clone(),
        kind: crate::task::FetchKind::parse(kind)?,
        expected_sha256,
    })))
}

/// The tail of a tag-11 witness entry: `[name, method]`, `[sha256?]` and
/// `[marker, (path, type)...]`. `None` for anything malformed, which is a
/// miss rather than a guess -- the alternative is replaying a *different*
/// filtered copy and calling its answer this one's.
fn question_filtered(root: String, rest: &[CanonValue]) -> Option<Question> {
    let (CanonValue::Array(head), CanonValue::Array(sha), CanonValue::Array(list)) =
        (rest.first()?, rest.get(1)?, rest.get(2)?)
    else {
        return None;
    };
    let (CanonValue::Str(name), CanonValue::Str(method)) = (head.first()?, head.get(1)?) else {
        return None;
    };
    let inherit_references = match head.get(2) {
        None => false,
        Some(CanonValue::Str(m)) if m == "inherit-references" => true,
        // A spelling this build does not know is a witness from a different
        // build. `None` makes that a miss, where guessing false would replay
        // a copy that inherited references as one that did not.
        Some(_) => return None,
    };
    let expected_sha256 = match sha.first() {
        None => None,
        Some(CanonValue::Str(h)) => Some(h.clone()),
        Some(_) => return None,
    };
    let CanonValue::Str(marker) = list.first()? else {
        return None;
    };
    let accepted = match marker.as_str() {
        "unfiltered" => None,
        "filtered" => {
            let tail = list.get(1..)?;
            if tail.len() % 2 != 0 {
                return None;
            }
            let mut out = Vec::with_capacity(tail.len() / 2);
            for pair in tail.chunks_exact(2) {
                let (CanonValue::Str(path), CanonValue::Str(kind)) = (pair.first()?, pair.get(1)?)
                else {
                    return None;
                };
                out.push(crate::task::AcceptedPath {
                    path: path.clone(),
                    file_type: file_type_from(kind)?,
                });
            }
            Some(out)
        }
        _ => return None,
    };
    Some(Question::StoreFiltered(Box::new(
        crate::task::FilteredCopy {
            root,
            name: name.clone(),
            method: crate::task::PathMethod::parse(method)?,
            accepted,
            expected_sha256,
            inherit_references,
        },
    )))
}

/// The inverse of [`FileType::as_str`]. `None` rather than `Unknown` for an
/// unrecognised spelling: a witness naming a type this build does not know is
/// a witness from a different build, and decoding it as `Unknown` would key a
/// cache row on a type nobody recorded.
fn file_type_from(name: &str) -> Option<FileType> {
    match name {
        "regular" => Some(FileType::Regular),
        "directory" => Some(FileType::Directory),
        "symlink" => Some(FileType::Symlink),
        "unknown" => Some(FileType::Unknown),
        _ => None,
    }
}

/// A resolved search path, digested. As with a store copy, the answer's
/// *class* is part of the digest: "no resolver" and "not found" are different
/// facts, and a replay that turned one into the other would hit on a cache
/// entry built under the other.
/// The digest of one [`Host::file_type`] answer.
///
/// Written once and called from both sides -- [`RecordingHost::file_type`]
/// records with it and [`Question::digest`] re-asks with it -- because two
/// spellings of one key is two chances for a recorded witness to stop
/// matching the replay that checks it, silently and in the direction of
/// "everything is still valid".
///
/// Three outcomes, three tags. `kind-absent` is its own tag rather than a
/// `kind-err` carrying "does not exist" text, because a read set that cannot
/// tell "the accessor has no such path" from "the read was refused" would
/// replay one as the other, and the two differ in exactly the case ENG-13123
/// was: absence is an ordinary answer that evaluation continues past.
fn digest_file_type(answer: &Result<Option<FileType>, String>) -> Hash {
    match answer {
        Ok(Some(kind)) => digest(&[b"kind-ok", kind.as_str().as_bytes()]),
        Ok(None) => digest(&[b"kind-absent"]),
        Err(error) => digest(&[b"kind-err", error.as_bytes()]),
    }
}

fn digest_find_file(answer: &Result<String, LookupError>) -> Hash {
    match answer {
        Ok(path) => digest(&[b"find-ok", path.as_bytes()]),
        Err(LookupError::NotFound(message)) => digest(&[b"find-miss", message.as_bytes()]),
        Err(LookupError::Failed(message)) => digest(&[b"find-err", message.as_bytes()]),
        Err(LookupError::Unsupported(message)) => {
            digest(&[b"find-unsupported", message.as_bytes()])
        }
        Err(LookupError::NoResolver) => digest(&[b"find-absent"]),
    }
}

fn digest_nix_path(answer: &Result<Vec<SearchPathEntry>, LookupError>) -> Hash {
    match answer {
        Ok(entries) => {
            let mut parts: Vec<Vec<u8>> = vec![b"nixpath-ok".to_vec()];
            for e in entries {
                parts.push(e.prefix.clone().into_bytes());
                parts.push(e.path.clone().into_bytes());
            }
            let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
            digest(&refs)
        }
        Err(
            LookupError::NotFound(message)
            | LookupError::Failed(message)
            | LookupError::Unsupported(message),
        ) => digest(&[b"nixpath-err", message.as_bytes()]),
        Err(LookupError::NoResolver) => digest(&[b"nixpath-absent"]),
    }
}

/// How loudly a cache complaint needs to be said.
///
/// Two levels and not one, because the existing channel carried everything at
/// the same volume and the two kinds are not the same news. A damaged row is
/// a slower run: the cache noticed, refused it, and re-evaluated, so nobody
/// got a wrong answer. A verifier disagreement is the cache having served an
/// answer that differs from what evaluating produces, which means something
/// already believed a wrong value.
///
/// Emitted at a real priority rather than as prose that opens with the word
/// "error": systemd hands journald `info` for any line without a syslog level
/// prefix, so a severity written into the message body is invisible to every
/// query that filters on one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// The cache cost something. No answer was wrong.
    Warning,
    /// The cache served an answer that re-evaluating does not reproduce, or
    /// could not serve one it should have. Somebody has to look.
    Error,
}

/// One thing the cache has to tell the embedder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Complaint {
    pub severity: Severity,
    pub message: String,
}

impl Complaint {
    #[must_use]
    pub fn warning(message: String) -> Self {
        Self {
            severity: Severity::Warning,
            message,
        }
    }

    #[must_use]
    pub fn error(message: String) -> Self {
        Self {
            severity: Severity::Error,
            message,
        }
    }
}

impl std::fmt::Display for Complaint {
    /// `severity: message`, with the severity first.
    ///
    /// Leading, not buried: a reader scanning a log and a journal filtering on
    /// priority both key on the label, and a sentence whose body happens to
    /// contain the word "error" is not a severity.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self.severity {
            Severity::Warning => "warning",
            Severity::Error => "error",
        };
        write!(f, "{label}: {}", self.message)
    }
}

/// What the sampling verifier found, counted so a run can be judged without
/// reading its log.
///
/// Separate counters for the two failure shapes, because they are different
/// bugs and a single "problems" total would hide which. A disagreement is a
/// wrong answer served. A miss that should have hit is a cache that is
/// correct and useless -- the shape that has now cost this repo twice, in the
/// `CopyToStore` decoder gap and again when a sweep deleted every witness
/// (ENG-12601), and the shape a verifier that only compares values on hits
/// cannot see at all.
/// # Three classes, three urgencies, deliberately not one total
///
/// A single "verifier failures" number would hide which fired, and they are
/// not equally bad:
///
/// - `hits_disagreed` is a **wrong answer already shipped**. Something
///   believed a value the evaluator does not produce.
/// - `records_not_replayable` is **lost speed**. Every answer was right; the
///   cache is paying to write rows it will never serve.
/// - a sweep post-condition failure is a **bug in the store**, and lives on
///   [`crate::store::SweepReport`] rather than here, because the sweep is
///   where both facts are and no in-process counter can see it. See the note
///   on `session::evaluate`'s hit side for why the three cannot be merged.
///
/// # Where these belong once the stats block exists
///
/// ENG-12546's part 2 adds a stats/histogram block to the C ABI. These four
/// counters and `SweepReport`'s two are meant to land in it as one accounting
/// path, keeping the classes distinguishable. That block is not merged at the
/// time of writing, so they are defined here, where they are produced, rather
/// than in a second block invented to hold them; the move is a rename, not a
/// redesign.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifierCounts {
    /// Hits that were re-evaluated and agreed.
    pub hits_checked: u64,
    /// Hits that were re-evaluated and did not agree.
    pub hits_disagreed: u64,
    /// Records that were looked up again in the same process and hit.
    pub records_checked: u64,
    /// Records that were looked up again in the same process and missed.
    ///
    /// Blind, by construction, to anything that destroys the store after this
    /// process exits: the witness is still on disk while this check runs.
    /// ENG-12601 was exactly that, and passes this check.
    pub records_not_replayable: u64,
}

/// Memoised evaluation results, keyed on the module and everything the
/// evaluation read.
///
/// The witness map is a hint and nothing more. Correctness rests on the key
/// being computed from answers observed now; a witness that no longer
/// describes what the evaluation would ask produces a key nothing was stored
/// under, hence a miss. See the module header.
pub struct ResultCache<'a, C: Cas + ?Sized> {
    cas: &'a C,
    table: MemoTable,
    lock: EffectLock,
    config: KernelConfig,
    witness: BTreeMap<EvalId, Vec<Question>>,
    /// Rows and witnesses that outlive the process. Both or neither: rows
    /// without witnesses is a cache holding answers it cannot address, and
    /// witnesses without rows is a set of reads that lead nowhere.
    rows: Option<&'a DirRows>,
    witness_store: Option<&'a DirWitness>,
    /// Corruption found while reading the store, drained by the caller. See
    /// the note on `ModuleCache::corruption`.
    corruption: Vec<Complaint>,
    /// How often a hit is re-evaluated and a record looked up again. 0 is off,
    /// 1 checks everything, N checks about one in N. See
    /// [`ResultCache::set_verify_rate`].
    verify_rate: u32,
    /// Xorshift state, advanced once per sampling decision.
    verify_state: u64,
    verifier: VerifierCounts,
    hits: u64,
    misses: u64,
    /// How many questions replay asked without the lookup then hitting. A
    /// replay that keeps failing is paying for a cache that never pays back,
    /// and it is invisible unless counted.
    wasted_replays: u64,
}

impl<'a, C: Cas + ?Sized> ResultCache<'a, C> {
    pub fn new(cas: &'a C) -> Self {
        Self {
            cas,
            table: MemoTable::new(),
            lock: EffectLock::new(),
            config: KernelConfig::default(),
            witness: BTreeMap::new(),
            rows: None,
            witness_store: None,
            corruption: Vec::new(),
            verify_rate: 0,
            // Any non-zero seed will do; the sequence only has to be spread,
            // not unpredictable. Fixed rather than time-derived so a failing
            // run can be repeated.
            verify_state: 0x2545_f491_4f6c_dd1d,
            verifier: VerifierCounts::default(),
            hits: 0,
            misses: 0,
            wasted_replays: 0,
        }
    }

    /// Open a cache warmed from a previous process's rows and witnesses.
    /// Open a cache backed by a previous process's rows and witnesses.
    ///
    /// Lazy for the same reason `ModuleCache::persistent` is: witnesses were
    /// already read one at a time, and reading every row up front made the
    /// two halves disagree about what a cold start should cost.
    pub fn persistent(cas: &'a C, rows: &'a DirRows, witness_store: &'a DirWitness) -> Self {
        let mut cache = Self::new(cas);
        cache.rows = Some(rows);
        cache.witness_store = Some(witness_store);
        cache
    }

    /// Record that an answer could not be memoised. Not an evaluation
    /// failure: the answer was right, the next run will just be slower.
    pub fn note_record_failure(&mut self, detail: String) {
        self.corruption.push(Complaint::warning(detail));
    }

    /// Check one hit in `rate` by evaluating anyway, and one record in `rate`
    /// by looking it up again. 0 turns it off; 1 checks every one.
    ///
    /// # Why sample at all, rather than check everything or nothing
    ///
    /// Checking everything costs a full evaluation per hit, which is the
    /// entire saving the cache exists for. Checking nothing is what shipped,
    /// and ENG-12541 -- a memo key blind to the store directory, so a cache
    /// shared across stores served paths for the wrong one -- would have been
    /// found in production by a one-in-twenty check and was instead found by
    /// reading the code. A rate is the only setting under which the cache is
    /// both worth having and watched.
    ///
    /// # What a sampled run returns
    ///
    /// The **served** answer, on both paths, even when the check disagreed
    /// with it. Not because it is the more trustworthy of the two -- it is
    /// the less -- but because it is what every unsampled run of the same
    /// expression gets, and a command whose output depended on whether the
    /// sampler happened to pick it would be the harder bug to chase. The
    /// disagreement leaves an error-priority complaint, which is the part
    /// that must not be missed.
    ///
    /// It costs the handle path more than it costs
    /// [`crate::session::evaluate`]. There the check is an extra `run` this
    /// crate performs; on the handle path the embedder has to redo its walk,
    /// so `capi::ixe_session_eval_question` hands back both a root handle and
    /// the served answer and asks for the fresh one back to compare against.
    /// `capi::warm_starts::a_sampled_hit_is_checked_rather_than_trusted`
    /// exercises that arm, which is otherwise dead code: the default rate is
    /// 0, so nothing in an ordinary run or in any gate enters it.
    pub fn set_verify_rate(&mut self, rate: u32) {
        self.verify_rate = rate;
    }

    /// Whether this occasion is one of the sampled ones.
    ///
    /// Xorshift rather than a counter: every Nth is predictable, and a
    /// workload whose shape lines up with N would sample the same expressions
    /// for ever and never look at the others. The seed is fixed, so a run
    /// that finds something can be repeated.
    pub fn should_verify(&mut self) -> bool {
        match self.verify_rate {
            0 => false,
            1 => true,
            rate => {
                let mut x = self.verify_state;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.verify_state = x;
                x.is_multiple_of(u64::from(rate))
            }
        }
    }

    /// The sampler's state, for a caller that builds a cache per call.
    ///
    /// [`crate::session::QuestionCache`] does, because the store it borrows
    /// outlives it, and without carrying this the draw restarts from the
    /// fixed seed every time: "one hit in N" becomes "every hit or no hit",
    /// the same in every process, and a sampler that is off looks exactly
    /// like one that is on and happening not to fire.
    #[must_use]
    pub fn verify_state(&self) -> u64 {
        self.verify_state
    }

    /// Resume the sampler where another cache over the same store left off.
    pub fn set_verify_state(&mut self, state: u64) {
        self.verify_state = state;
    }

    /// What the verifier has seen. Counted even when nothing went wrong, so a
    /// run can tell "checked and agreed" from "never checked" -- the two look
    /// identical in a log that only speaks up on failure.
    #[must_use]
    pub fn verifier(&self) -> VerifierCounts {
        self.verifier
    }

    /// A sampled hit was re-evaluated and agreed.
    pub fn note_hit_verified(&mut self) {
        self.verifier.hits_checked += 1;
    }

    /// A sampled hit was re-evaluated and did not agree. The cache served an
    /// answer that evaluating does not reproduce.
    pub fn note_hit_disagreed(&mut self, identity: &EvalId, served: &str, fresh: &str) {
        self.verifier.hits_checked += 1;
        self.verifier.hits_disagreed += 1;
        self.corruption.push(Complaint::error(format!(
            "the evaluation cache served an answer that re-evaluating does not \
             reproduce. memo key {}: served {served:?}, evaluating now gives \
             {fresh:?}. Every answer this cache has given for this key is \
             suspect; the key is printed so the row can be found and the \
             inputs that differ identified.",
            identity.as_hash().to_hex()
        )));
    }

    /// A sampled record was looked up again in the same process, as it must
    /// be for the cache to ever pay back.
    pub fn note_record_replayable(&mut self, replayable: bool, identity: &EvalId) {
        self.verifier.records_checked += 1;
        if !replayable {
            self.verifier.records_not_replayable += 1;
            self.corruption.push(Complaint::error(format!(
                "the evaluation cache recorded a result and then could not \
                 find it again in the same process. memo key {}: everything \
                 filed under this key is unreachable, so the cache is paying \
                 to write answers it will never serve. This is what a key that \
                 differs between the record and lookup paths looks like, and \
                 what an unreadable witness looks like.",
                identity.as_hash().to_hex()
            )));
        }
    }

    /// Take the corruption found since the last call.
    pub fn take_corruption(&mut self) -> Vec<Complaint> {
        core::mem::take(&mut self.corruption)
    }

    #[must_use]
    pub fn hits(&self) -> u64 {
        self.hits
    }

    #[must_use]
    pub fn misses(&self) -> u64 {
        self.misses
    }

    #[must_use]
    pub fn wasted_replays(&self) -> u64 {
        self.wasted_replays
    }

    /// Try to answer without evaluating.
    ///
    /// Asks the host the questions last time's evaluation asked, keys on the
    /// answers it gets *now*, and returns a stored result only if one exists
    /// under exactly that key.
    pub fn lookup(
        &mut self,
        identity: &EvalId,
        host: &dyn Host,
        settings: &crate::eval::Settings,
    ) -> Option<EvalResult> {
        // Witnesses are read lazily rather than all at load: a store can hold
        // far more evaluations than one process will ask about.
        let questions = match self.witness.get(identity) {
            Some(questions) => questions.clone(),
            None => {
                let questions = self.witness_store?.get(identity)?;
                self.witness.insert(*identity, questions.clone());
                questions
            }
        };
        let observed = ReadSet::replay(&questions, host, settings)?;
        let key = row_key(&observed.key(identity)).ok()?;
        // Bring the row in from disk if this process has not seen it.
        if self.table.get(eval_domain(), key).is_none()
            && let Some(rows) = self.rows
        {
            match rows.get(eval_domain(), key) {
                Lookup::Missing => {}
                Lookup::Refused(reason) => {
                    self.corruption.push(Complaint::warning(reason.to_string()))
                }
                Lookup::Found(output) => {
                    self.table.insert(
                        eval_domain(),
                        key,
                        ix_kernel::Entry {
                            output,
                            policy: Policy::Keyed,
                            provenance: ix_kernel::Provenance::Deterministic,
                        },
                    );
                }
            }
        }
        let entry = self.table.get(eval_domain(), key)?;
        let bytes = self.cas.get(entry.output).ok().flatten()?;
        // The address does not vouch for the bytes on its own: DirCas names
        // files by address and does not re-hash on read.
        if ix_kernel::ObjId::of(&bytes) != entry.output {
            self.corruption.push(Complaint::warning(format!(
                "object {} for a memoised result does not hash to its address; re-evaluating",
                entry.output
            )));
            self.wasted_replays += 1;
            return None;
        }
        match decode_result(&bytes) {
            Some(result) => {
                self.hits += 1;
                if let Some(rows) = self.rows {
                    rows.touch(eval_domain(), key);
                }
                Some(result)
            }
            None => {
                // A stored object that is not a result is corruption, not a
                // hit; re-evaluating is always safe under Keyed.
                self.corruption.push(Complaint::warning(format!(
                    "object {} is not a memoised result; re-evaluating",
                    entry.output
                )));
                self.wasted_replays += 1;
                None
            }
        }
    }

    /// Called when a lookup missed and the evaluation ran, so the counters can
    /// tell a first sighting from a replay that did not pay off.
    pub fn note_miss(&mut self, identity: &EvalId) {
        self.misses += 1;
        if self.witness.contains_key(identity) {
            self.wasted_replays += 1;
        }
    }

    /// Record what an evaluation read and what it produced.
    pub fn record(
        &mut self,
        identity: &EvalId,
        read_set: &ReadSet,
        result: &EvalResult,
    ) -> Result<(), KernelError> {
        let encoded = request_bytes(&read_set.key(identity))?;
        let payload = encode_result(result)?;
        let performed = on_perform(
            PerformCtx {
                table: &mut self.table,
                lock: &mut self.lock,
                cas: self.cas,
                config: &self.config,
                performed_at: "",
                blessed_by: "",
            },
            eval_domain(),
            &Policy::Keyed,
            &encoded,
            || Ok::<_, KernelError>(payload),
        )?;
        let questions = read_set.questions();
        if let Some(store) = self.witness_store {
            // A witness that cannot be written costs future hits and nothing
            // else, so it must not fail the evaluation that produced it.
            if let Err(error) = store.put(identity, &questions) {
                return Err(KernelError::Io {
                    doing: "recording an evaluation witness".to_owned(),
                    source: error,
                });
            }
        }
        if let Some(rows) = self.rows {
            rows.put(eval_domain(), &encoded, performed.output)?;
        }
        self.witness.insert(*identity, questions);
        Ok(())
    }
}

/// The canonical request bytes a result row is keyed by.
///
/// Both the lookup and the record path go through this. They used to build
/// the key separately, one from the raw digest and one from its canonical
/// encoding, so every lookup missed while every store succeeded: the cache
/// was correct, cost extra, and never once paid back. Correct-but-useless is
/// invisible to a gate that only compares answers, which is why the harness
/// counts how many answers came from the cache and not just whether they
/// agreed.
fn request_bytes(key: &Hash) -> Result<Vec<u8>, KernelError> {
    Ok(canon::encode(&CanonValue::Bytes(key.as_bytes().to_vec()))?)
}

fn row_key(key: &Hash) -> Result<ix_kernel::Key, KernelError> {
    Ok(ix_kernel::Key::mint(eval_domain(), &request_bytes(key)?))
}

/// `Key::mint` hashes the canonical request, and the request here is just the
/// read-set key, so this is the one place the two hashings meet.
fn encode_result(result: &EvalResult) -> Result<Vec<u8>, KernelError> {
    Ok(canon::encode(&CanonValue::map([
        ("status", CanonValue::str(result.status.as_str())),
        ("value", CanonValue::str(result.value.as_str())),
        (
            // Empty means "not a refusal", which is not the same as the key
            // being absent; absent is a row written before tokens existed,
            // and `decode_result` keeps the two apart.
            "token",
            CanonValue::str(result.token.map_or("", |t| t.as_str())),
        ),
        (
            // A failure with no position writes an empty array rather than
            // omitting the key, so a row that has one and a row from before
            // positions existed stay distinguishable on the way back in.
            "pos",
            match &result.pos {
                None => CanonValue::array([]),
                Some(pos) => CanonValue::array([
                    CanonValue::str(pos.file.as_deref().unwrap_or("")),
                    CanonValue::str(pos.line.to_string()),
                    CanonValue::str(pos.column.to_string()),
                ]),
            },
        ),
        (
            "emissions",
            CanonValue::array(result.emissions.iter().map(|emission| {
                let (kind, message) = emission.parts();
                CanonValue::array([CanonValue::str(kind), CanonValue::str(message.as_str())])
            })),
        ),
    ]))?)
}

fn decode_result(bytes: &[u8]) -> Option<EvalResult> {
    let CanonValue::Map(entries) = canon::decode(bytes).ok()? else {
        return None;
    };
    let field = |name: &str| {
        entries.iter().find_map(|(k, v)| match (k, v) {
            (CanonValue::Str(k), CanonValue::Str(v)) if k == name => Some(v.clone()),
            _ => None,
        })
    };
    // A row written before emissions were part of a result has none of this
    // key. Treated as "said nothing" rather than as corruption: the value it
    // holds is still right, and the alternative is discarding every row in an
    // existing store to gain nothing.
    let emissions = entries.iter().find_map(|(k, v)| match (k, v) {
        (CanonValue::Str(k), CanonValue::Array(items)) if k == "emissions" => Some(items),
        _ => None,
    });
    let emissions = match emissions {
        None => Vec::new(),
        Some(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                // Anything that is not a `(kind, message)` pair of strings
                // this build knows is a damaged row, and `lookup` treats the
                // `None` as corruption rather than serving a partial answer.
                let CanonValue::Array(pair) = item else {
                    return None;
                };
                let (Some(CanonValue::Str(kind)), Some(CanonValue::Str(message))) =
                    (pair.first(), pair.get(1))
                else {
                    return None;
                };
                out.push(Emission::from_parts(kind, message.clone())?);
            }
            out
        }
    };
    // A row from before tokens existed carries no key at all, which is not
    // the same as a row that has one and is not a refusal. It reads back as
    // `Unrecorded` so a census can see the size of the population it cannot
    // classify, instead of counting it as "no refusal" -- a wrong answer
    // rather than a missing one.
    let status = field("status")?;
    let token = match field("token") {
        None if status == crate::session::UNIMPLEMENTED => {
            Some(crate::refusal::RefusalToken::Unrecorded)
        }
        None => None,
        Some(name) if name.is_empty() => None,
        Some(name) => Some(
            crate::refusal::RefusalToken::parse(&name)
                .unwrap_or(crate::refusal::RefusalToken::Unrecorded),
        ),
    };
    // A row written before positions existed has no key at all, and one
    // written for a failure with no position has an empty array. Both mean
    // "no position"; a malformed triple is a damaged row and refuses, the
    // way a malformed emission does.
    let pos = match entries.iter().find_map(|(k, v)| match (k, v) {
        (CanonValue::Str(k), CanonValue::Array(items)) if k == "pos" => Some(items),
        _ => None,
    }) {
        None => None,
        Some(items) if items.is_empty() => None,
        Some(items) => {
            let (
                Some(CanonValue::Str(file)),
                Some(CanonValue::Str(line)),
                Some(CanonValue::Str(column)),
            ) = (items.first(), items.get(1), items.get(2))
            else {
                return None;
            };
            let (Ok(line), Ok(column)) = (line.parse(), column.parse()) else {
                return None;
            };
            Some(crate::vm::SrcPos {
                file: if file.is_empty() {
                    None
                } else {
                    Some(std::rc::Rc::from(file.as_str()))
                },
                line,
                column,
            })
        }
    };
    Some(EvalResult {
        status,
        value: field("value")?,
        emissions,
        token,
        pos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Fake {
        contents: Cell<u8>,
    }
    impl Host for Fake {
        crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
        fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
            self.read_file(path).map(String::into_bytes)
        }
        crate::host::host_stubs!(
            realise,
            store_text,
            write_derivation,
            store_filtered,
            fetch,
            lock_flake,
            fetch_tree,
            not_async,
        );
        crate::host::host_stubs!(
            file_type_resolved,
            copy_to_store,
            ensure_path,
            warn,
            find_file,
            nix_path,
            trace
        );
        fn read_file(&self, path: &str) -> Result<String, String> {
            match path {
                "/a" => Ok(format!("v{}", self.contents.get())),
                _ => Err(format!("path '{path}' does not exist")),
            }
        }
        fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
            Ok(vec![("x".to_owned(), FileType::Regular)])
        }
        fn path_exists(&self, path: &str) -> bool {
            path == "/a"
        }
        fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
            Ok(Some(FileType::Regular))
        }
        fn get_env(&self, name: &str) -> Option<String> {
            match name {
                "SET" => Some("yes".to_owned()),
                _ => None,
            }
        }
    }

    fn fake() -> Fake {
        Fake {
            contents: Cell::new(1),
        }
    }

    /// Replay under the default settings, where reads are allowed, so the
    /// refusal `ReadSet::replay` returns under `pure-eval` is not silently
    /// swallowed as an empty read set. A test that meant to exercise the
    /// refusal asserts on the `None` itself.
    fn replayed(questions: &[Question], host: &dyn Host) -> ReadSet {
        match ReadSet::replay(questions, host, &crate::eval::Settings::default()) {
            Some(set) => set,
            None => unreachable!("replay refused; a purity setting is on in this test"),
        }
    }

    /// An evaluation identity for a made-up module under a stated
    /// configuration.
    ///
    /// `Settings::default()` and not `Settings::current()`, which is the
    /// whole of why `the_module_is_part_of_the_key` used to fail about once
    /// in fifty runs: the identity was built from the process settings, so a
    /// test moving `pure-eval` between this call and the next changed the key
    /// out from under a lookup that had to hit. That the settings are part of
    /// the key is the correct behaviour being tested elsewhere; a test about
    /// the *module* half has no business varying the settings half.
    fn id(module: &[u8]) -> EvalId {
        EvalId::of(
            &hash::tagged("m", &[module]),
            &crate::eval::Settings::default(),
            // These tests vary the read set and hold the arguments and the
            // question constant; those axes have their own tests in `session`.
            &crate::session::Arguments::none(),
            &crate::session::Question::Whole {
                render: crate::session::RenderMode::Plain,
            },
        )
    }

    /// The list of samples covers the enum, and the compiler makes it stay
    /// that way. See [`Question::variant_index`] for the chain this is step 2
    /// and 3 of.
    #[test]
    fn every_question_variant_is_listed() {
        let all = Question::one_of_each();
        let mut seen = [false; Question::VARIANT_COUNT];
        for question in &all {
            let index = question.variant_index();
            assert!(
                index < Question::VARIANT_COUNT,
                "{question:?} has index {index}, past VARIANT_COUNT \
                 {}: raise the count and add a sample below it",
                Question::VARIANT_COUNT
            );
            let Some(slot) = seen.get_mut(index) else {
                // Unreachable given the bound asserted above; written as a
                // refusal rather than an index because the workspace denies
                // `indexing_slicing`, tests included.
                unreachable!("variant index {index} is out of range");
            };
            assert!(
                !*slot,
                "two samples share variant index {index}; one variant is \
                 covered twice and another not at all"
            );
            *slot = true;
        }
        let missing: Vec<usize> = seen
            .iter()
            .enumerate()
            .filter_map(|(i, hit)| (!hit).then_some(i))
            .collect();
        assert!(
            missing.is_empty(),
            "variant indexes {missing:?} have no sample in one_of_each, so \
             nothing round-trips them through the witness codec"
        );
    }

    /// ENG-12543. Replaying a witness must not do the reads `pure-eval` and
    /// `restrict-eval` forbid.
    ///
    /// `Question::ask` calls the host directly and the evaluator's access
    /// check lives in `eval::answer_path`, which replay never enters, so this
    /// path had none. Measured at 8065be845, before the settings reached the
    /// memo key, a cache filled with reads allowed and then looked up under
    /// `pure-eval` returned `status=ok value="secret" memo_hit=true
    /// reads=["/etc/shadow"]` -- the setting bypassed in both directions.
    #[test]
    fn replay_refuses_to_read_when_access_is_off() {
        struct Counting {
            reads: RefCell<Vec<String>>,
        }
        impl Host for Counting {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                copy_to_store,
                ensure_path,
                warn,
                trace,
                find_file,
                nix_path
            );
            fn read_file(&self, path: &str) -> Result<String, String> {
                self.reads.borrow_mut().push(path.to_owned());
                Ok("secret".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, path: &str) -> bool {
                self.reads.borrow_mut().push(format!("exists {path}"));
                true
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Ok(Some(FileType::Regular))
            }
            fn get_env(&self, name: &str) -> Option<String> {
                self.reads.borrow_mut().push(format!("env {name}"));
                Some("v".to_owned())
            }
        }

        let host = Counting {
            reads: RefCell::new(Vec::new()),
        };
        let questions = vec![
            Question::ReadFile("/etc/shadow".to_owned()),
            Question::GetEnv("SECRET".to_owned()),
            Question::PathExists("/private".to_owned()),
        ];

        // Stated, not installed. This used to set the process-global
        // `pure-eval` and put it back, under a write guard, which made every
        // unguarded reader in the suite race with it (ENG-12939).
        let pure = crate::eval::Settings {
            pure_eval: true,
            ..crate::eval::Settings::default()
        };
        let refused = ReadSet::replay(&questions, &host, &pure);
        let read_with_access_off = host.reads.borrow().clone();

        assert!(
            read_with_access_off.is_empty(),
            "replay reached the world with access off: {read_with_access_off:?}"
        );
        assert!(
            refused.is_none(),
            "replay must refuse rather than return an empty read set, which \
             would key as though the questions had been asked and answered"
        );

        // And with access on it still works, so the guard is a refusal and
        // not a permanent disabling of replay.
        assert!(ReadSet::replay(&questions, &host, &crate::eval::Settings::default()).is_some());
        assert_eq!(host.reads.borrow().len(), 3);
    }

    /// ENG-12792, the other direction. With the embedder's read hooks
    /// installed the same three questions replay under `pure-eval`, because
    /// the reads then go through cppnix's `rootFS` and the setting is
    /// enforced there.
    ///
    /// Without this the change would be invisible here: the test above
    /// asserts a refusal and would keep passing if the five rows never moved.
    /// Two tests, one per configuration, is what says the table is a
    /// decision rather than a constant.
    ///
    /// The fixture host is a leaf and not `RealFs`, so the hooks do not
    /// change what it answers -- only whether replay is allowed to ask. That
    /// split is the point being tested. In the `nix` binary the two are the
    /// same object: the host chain ends in `RealFs`, which is where the hooks
    /// are read.
    #[test]
    fn replay_reads_again_when_the_embedder_answers() {
        struct Counting {
            reads: RefCell<Vec<String>>,
        }
        impl Host for Counting {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                copy_to_store,
                ensure_path,
                warn,
                trace,
                find_file,
                nix_path
            );
            fn read_file(&self, path: &str) -> Result<String, String> {
                self.reads.borrow_mut().push(path.to_owned());
                Ok("from the accessor".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, path: &str) -> bool {
                self.reads.borrow_mut().push(format!("exists {path}"));
                true
            }
            fn file_type(&self, path: &str) -> Result<Option<FileType>, String> {
                self.reads.borrow_mut().push(format!("kind {path}"));
                Ok(Some(FileType::Regular))
            }
            fn get_env(&self, _name: &str) -> Option<String> {
                None
            }
        }

        let host = Counting {
            reads: RefCell::new(Vec::new()),
        };
        let questions = vec![
            Question::ReadFile("/nix/store/aaa-source/flake.nix".to_owned()),
            Question::PathExists("/nix/store/aaa-source".to_owned()),
            Question::FileType("/nix/store/aaa-source".to_owned()),
        ];

        // Two configurations named as values. This test used to install real
        // read hooks and clear them again purely to move
        // `PathReads::current()`, which meant a fake filesystem was briefly
        // visible to every other test in the process; now that `path_reads`
        // is a field of `Settings` there is nothing to install (ENG-12939).
        let standalone_settings = crate::eval::Settings {
            pure_eval: true,
            path_reads: crate::purity::PathReads::Direct,
            ..crate::eval::Settings::default()
        };
        let bridged_settings = crate::eval::Settings {
            path_reads: crate::purity::PathReads::ThroughEmbedder,
            ..standalone_settings.clone()
        };

        let standalone = ReadSet::replay(&questions, &host, &standalone_settings);
        let bridged = ReadSet::replay(&questions, &host, &bridged_settings);
        let asked = host.reads.borrow().clone();

        assert!(
            standalone.is_none(),
            "a std::fs read set must not replay under pure-eval"
        );
        assert!(
            bridged.is_some(),
            "with the embedder's read hooks installed these three go through \
             cppnix's rootFS, so replay has to ask them; refusing is the \
             pre-ENG-12792 behaviour and makes every flake witness unusable"
        );
        assert_eq!(asked.len(), 3, "replay asked {asked:?}");
    }

    /// A witness that names nothing still replays, so a pure expression keeps
    /// its cache under `pure-eval` -- the one case where caching is
    /// unambiguously safe. A guard that refused this would trade a real
    /// speedup for no safety.
    #[test]
    fn an_empty_witness_still_replays_with_access_off() {
        struct Nothing;
        impl Host for Nothing {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                get_env,
                copy_to_store,
                ensure_path,
                warn,
                trace,
                find_file,
                nix_path
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                false
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Err("no".to_owned())
            }
        }

        let pure = crate::eval::Settings {
            pure_eval: true,
            ..crate::eval::Settings::default()
        };
        let replayed = ReadSet::replay(&[], &Nothing, &pure);
        assert!(replayed.is_some_and(|set| set.is_empty()));
    }

    /// The property that stops the `CopyToStore` bug recurring in a new
    /// variant's name.
    ///
    /// Every variant, not a list somebody maintains: `one_of_each` is
    /// generated from the same declaration the codec is, so a variant added
    /// tomorrow is covered here the moment it exists. What this would have
    /// caught is exactly what happened -- `CopyToStore` encoded to tag 6 and
    /// decoded to `None`, so every witness naming one was unreadable and
    /// every evaluation containing `"${./x}"` re-evaluated for ever.
    #[test]
    fn every_question_variant_round_trips_through_the_witness_codec() {
        let all = Question::one_of_each();
        assert!(all.len() >= 7, "the enum shrank: {all:?}");
        for question in &all {
            let encoded = question_value(question);
            let decoded = question_from(&encoded);
            assert_eq!(
                decoded.as_ref(),
                Some(question),
                "{question:?} does not survive the witness codec, so it can never cache-hit"
            );
        }
    }

    /// Two variants sharing a tag would make one decode as the other, which
    /// is a wrong question replayed rather than a miss.
    #[test]
    fn no_two_question_variants_share_a_tag() {
        let all = Question::one_of_each();
        let mut tags: Vec<u8> = all.iter().map(Question::tag).collect();
        tags.sort_unstable();
        let mut unique = tags.clone();
        unique.dedup();
        assert_eq!(tags, unique, "duplicate question tags: {tags:?}");
        assert!(!tags.contains(&0), "tag 0 is reserved for 'absent'");
    }

    /// A whole witness, not one question at a time: the list codec has its own
    /// failure mode, where one unreadable entry discards the entire list.
    #[test]
    fn a_witness_naming_every_question_reads_back() -> Result<(), Box<dyn core::error::Error>> {
        let dir = std::env::temp_dir().join(format!(
            "ixe-witness-roundtrip-{}-{}",
            std::process::id(),
            WITNESS_COUNTER.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
        ));
        let store = DirWitness::open(&dir)?;
        let questions = Question::one_of_each();
        let identity = id(b"every-question");
        store.put(&identity, &questions)?;
        let back = store.get(&identity);
        std::fs::remove_dir_all(&dir)?;
        assert_eq!(back.as_ref(), Some(&questions));
        Ok(())
    }

    /// ENG-12540 (2). `RecordingHost` implemented six of `Host`'s eight
    /// methods and inherited the other two from the trait, so `ensure_path`
    /// answered "no store here" against a host that had one, and every
    /// warning was dropped.
    ///
    /// # What this still guards, now that the trait has no defaults
    ///
    /// Presence is no longer this test's job. Since ENG-13107 every effect on
    /// `Host` is bodiless, so a recorder that forgets one does not compile,
    /// and the sibling test that used to hold `ThreadedHost` and `&T` to a
    /// hand-maintained list of method names is gone.
    ///
    /// What survives is the half the compiler cannot check. This wrapper does
    /// not merely forward: every method here has a real body that records the
    /// question and *then* asks the inner host, and a body that records
    /// without asking compiles perfectly. That is a plausible mistake -- it
    /// is what a recorder does for a question it can answer from its own log
    /// -- and it would strand the effect. So this drives each effect through
    /// the recorder and names the call it expects to arrive at the host
    /// behind it.
    #[test]
    fn the_recorder_forwards_every_effect_to_the_host_behind_it() {
        struct Inner {
            asked: RefCell<Vec<String>>,
        }
        impl Inner {
            fn note(&self, what: &str) {
                self.asked.borrow_mut().push(what.to_owned());
            }
        }
        impl Host for Inner {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(realise, lock_flake, not_async);
            crate::host::host_stubs!(file_type_resolved, find_file, nix_path);
            fn read_file(&self, path: &str) -> Result<String, String> {
                self.note(&format!("read_file {path}"));
                Ok("text".to_owned())
            }
            fn read_dir(&self, path: &str) -> Result<Vec<(String, FileType)>, String> {
                self.note(&format!("read_dir {path}"));
                Ok(Vec::new())
            }
            fn path_exists(&self, path: &str) -> bool {
                self.note(&format!("path_exists {path}"));
                true
            }
            fn file_type(&self, path: &str) -> Result<Option<FileType>, String> {
                self.note(&format!("file_type {path}"));
                Ok(Some(FileType::Regular))
            }
            fn get_env(&self, name: &str) -> Option<String> {
                self.note(&format!("get_env {name}"));
                Some("v".to_owned())
            }
            fn copy_to_store(&self, path: &str) -> Result<String, StoreError> {
                self.note(&format!("copy_to_store {path}"));
                Ok("/nix/store/xyz".to_owned())
            }
            fn ensure_path(&self, path: &str) -> Result<(), StoreError> {
                self.note(&format!("ensure_path {path}"));
                Ok(())
            }
            fn store_text(
                &self,
                name: &str,
                _contents: &str,
                _references: &[String],
            ) -> Result<String, StoreError> {
                self.note(&format!("store_text {name}"));
                Ok("/nix/store/text".to_owned())
            }
            fn write_derivation(
                &self,
                name: &str,
                _aterm: &str,
                _references: &[String],
            ) -> Result<String, StoreError> {
                self.note(&format!("write_derivation {name}"));
                Ok("/nix/store/a.drv".to_owned())
            }
            fn store_filtered(
                &self,
                request: &crate::task::FilteredCopy,
            ) -> Result<String, StoreError> {
                self.note(&format!("store_filtered {}", request.root));
                Ok("/nix/store/filtered".to_owned())
            }
            fn fetch(&self, request: &crate::task::FetchRequest) -> Result<String, StoreError> {
                self.note(&format!("fetch {}", request.url));
                Ok("/nix/store/fetched".to_owned())
            }
            fn fetch_tree(
                &self,
                request: &crate::task::FetchTreeRequest,
            ) -> Result<String, StoreError> {
                self.note(&format!("fetch_tree {}", request.fetcher.as_str()));
                Ok("{}".to_owned())
            }
            fn warn(&self, message: &str) {
                self.note(&format!("warn {message}"));
            }
            fn trace(&self, message: &str) {
                self.note(&format!("trace {message}"));
            }
        }

        let inner = Inner {
            asked: RefCell::new(Vec::new()),
        };
        let host = RecordingHost::new(&inner);
        drop(host.read_file("/f"));
        drop(host.read_dir("/d"));
        let _ = host.path_exists("/e");
        drop(host.file_type("/t"));
        drop(host.get_env("V"));
        drop(host.copy_to_store("/c"));
        // The two that were inherited. Both assertions are the divergence:
        // before the fix these answered `Err(NoStore)` and silence.
        assert_eq!(host.ensure_path("/p"), Ok(()));
        // The store effects. The compiler now insists the recorder define
        // each of these, so what is being checked below is not that they
        // exist but that each one's body reaches the inner host rather than
        // stopping at the log. `resolve_import` is the one method left with a
        // trait default and is deliberately not overridden -- it is derived
        // from `file_type_resolved`, which is forwarded and recorded, so
        // inheriting it records the same question either way.
        drop(host.store_text("t", "bytes", &[]));
        drop(host.write_derivation("a", "Derive([])", &[]));
        drop(host.store_filtered(&crate::task::FilteredCopy {
            root: "/s".to_owned(),
            name: "s".to_owned(),
            method: crate::task::PathMethod::NixArchive,
            accepted: None,
            expected_sha256: None,
            inherit_references: false,
        }));
        drop(host.fetch(&crate::task::FetchRequest {
            url: "https://u/x".to_owned(),
            name: "x".to_owned(),
            kind: crate::task::FetchKind::File,
            expected_sha256: None,
        }));
        drop(host.fetch_tree(&crate::task::FetchTreeRequest {
            attrs: std::collections::BTreeMap::new(),
            fetcher: crate::task::TreeFetcher::Tree,
        }));
        host.warn("a warning cppnix would print");
        host.trace("a trace line builtins.trace would print");

        assert_eq!(
            *inner.asked.borrow(),
            vec![
                "read_file /f",
                "read_dir /d",
                "path_exists /e",
                "file_type /t",
                "get_env V",
                "copy_to_store /c",
                "ensure_path /p",
                "store_text t",
                "write_derivation a",
                "store_filtered /s",
                "fetch https://u/x",
                "fetch_tree fetchTree",
                "warn a warning cppnix would print",
                "trace a trace line builtins.trace would print",
            ],
            "an effect did not reach the host behind the recorder"
        );
        // Twelve effects are questions; the warning and the trace are outputs
        // and are kept separately, because they have no answer to key on.
        assert_eq!(host.take().len(), 12);
        assert_eq!(
            host.take_emissions(),
            vec![
                Emission::Warn("a warning cppnix would print".to_owned()),
                Emission::Trace("a trace line builtins.trace would print".to_owned()),
            ]
        );
    }

    /// `ensure_path`'s answer decides whether `builtins.appendContext`
    /// succeeds, so it is a question and a changed answer has to move the key.
    #[test]
    fn a_store_that_stops_producing_a_path_changes_the_key() {
        struct Store(Cell<bool>);
        impl Host for Store {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                get_env,
                copy_to_store,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                false
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Err("no".to_owned())
            }
            fn ensure_path(&self, _p: &str) -> Result<(), StoreError> {
                if self.0.get() {
                    Ok(())
                } else {
                    Err(StoreError::Failed("path is not valid".to_owned()))
                }
            }
        }
        let inner = Store(Cell::new(true));
        let host = RecordingHost::new(&inner);
        drop(host.ensure_path("/nix/store/xyz"));
        let recorded = host.take();
        let identity = id(b"ensure");
        inner.0.set(false);
        assert_ne!(
            recorded.key(&identity),
            replayed(&recorded.questions(), &inner).key(&identity)
        );
    }

    /// The token has to survive the store, or a served refusal is counted
    /// under the wrong kind for the rest of the cache's life -- and unlike a
    /// wrong value, nothing downstream can tell.
    #[test]
    fn the_refusal_token_survives_the_result_codec() -> Result<(), Box<dyn core::error::Error>> {
        let result = EvalResult {
            status: crate::session::UNIMPLEMENTED.to_owned(),
            value: "effect domain 'x'".to_owned(),
            emissions: Vec::new(),
            token: Some(crate::refusal::RefusalToken::EffectDomain),
            pos: None,
        };
        let back = decode_result(&encode_result(&result)?).ok_or("did not decode")?;
        assert_eq!(back.token, Some(crate::refusal::RefusalToken::EffectDomain));
        assert_eq!(back.status, result.status);
        Ok(())
    }

    /// A row written before the token existed has no key at all, and that is
    /// a different fact from a row that has one and is not a refusal. Reading
    /// the first as `Unrecorded` is what lets a census report the size of the
    /// population it cannot classify instead of quietly calling it "no
    /// refusal", which would be a wrong answer rather than a missing one.
    #[test]
    fn a_row_written_before_tokens_reads_as_unrecorded() -> Result<(), Box<dyn core::error::Error>>
    {
        // The pre-token encoding, written out by hand: status and value and
        // emissions, and no `token` key.
        let bytes = canon::encode(&CanonValue::map([
            ("status", CanonValue::str(crate::session::UNIMPLEMENTED)),
            ("value", CanonValue::str("something old")),
            ("emissions", CanonValue::array([])),
        ]))?;
        let back = decode_result(&bytes).ok_or("did not decode")?;
        assert_eq!(back.token, Some(crate::refusal::RefusalToken::Unrecorded));

        // ... while a non-refusal row with no key is simply not a refusal.
        let ok = canon::encode(&CanonValue::map([
            ("status", CanonValue::str(crate::session::OK)),
            ("value", CanonValue::str("1")),
            ("emissions", CanonValue::array([])),
        ]))?;
        assert_eq!(decode_result(&ok).ok_or("did not decode")?.token, None);
        Ok(())
    }

    /// A result carries what the evaluation said, so a served answer can say
    /// it again -- warnings and traces alike, in the order they happened.
    #[test]
    fn emissions_survive_the_result_codec() -> Result<(), Box<dyn core::error::Error>> {
        let result = EvalResult {
            status: "ok".to_owned(),
            value: "1".to_owned(),
            token: None,
            pos: None,
            emissions: vec![
                Emission::Warn("first".to_owned()),
                Emission::Trace("second".to_owned()),
            ],
        };
        assert_eq!(
            decode_result(&encode_result(&result)?).as_ref(),
            Some(&result)
        );
        Ok(())
    }

    /// A row written before emissions existed still decodes, with none. The
    /// alternative -- treating a missing key as corruption -- would discard
    /// every row in every existing store to gain nothing.
    #[test]
    fn a_result_row_without_emissions_decodes_as_having_none()
    -> Result<(), Box<dyn core::error::Error>> {
        let old = canon::encode(&CanonValue::map([
            ("status", CanonValue::str("ok")),
            ("value", CanonValue::str("1")),
        ]))?;
        assert_eq!(
            decode_result(&old),
            Some(EvalResult {
                status: "ok".to_owned(),
                value: "1".to_owned(),
                emissions: Vec::new(),
                token: None,
                pos: None,
            })
        );
        Ok(())
    }

    /// An `emissions` key holding something other than `(kind, text)` pairs
    /// is a damaged row, and a damaged row is a miss rather than half an
    /// answer.
    #[test]
    fn a_result_row_with_a_malformed_emission_list_is_refused()
    -> Result<(), Box<dyn core::error::Error>> {
        let bad = canon::encode(&CanonValue::map([
            ("status", CanonValue::str("ok")),
            ("value", CanonValue::str("1")),
            ("emissions", CanonValue::array([CanonValue::int(7)])),
        ]))?;
        assert_eq!(decode_result(&bad), None);
        Ok(())
    }

    #[test]
    fn the_recorder_logs_every_kind_of_question_in_order() {
        let inner = fake();
        let host = RecordingHost::new(&inner);
        drop(host.read_file("/a"));
        let _ = host.path_exists("/a");
        drop(host.get_env("SET"));
        let read_set = host.take();
        assert_eq!(read_set.len(), 3);
        assert_eq!(
            read_set.questions(),
            vec![
                Question::ReadFile("/a".to_owned()),
                Question::PathExists("/a".to_owned()),
                Question::GetEnv("SET".to_owned()),
            ]
        );
        // Taking empties it, so the next evaluation starts clean.
        assert!(host.take().is_empty());
    }

    /// An `import`'s world-read has to land in the log, and under the name of
    /// the question that was actually asked.
    ///
    /// `Host::resolve_import` keeps its default body on `RecordingHost`, so
    /// what records the read is whichever method that body calls. When it
    /// called `file_type` the log said `FileType`; it now calls
    /// `file_type_resolved` and the log has to say `FileTypeResolved`, or a
    /// replay would ask the `lstat` and key on an answer the evaluation never
    /// saw. Break `RecordingHost::file_type_resolved`'s `note` and this is
    /// what fails.
    #[test]
    fn an_import_records_the_resolving_kind_question_and_not_the_plain_one() {
        struct Dir;
        impl Host for Dir {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                get_env,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _path: &str) -> Result<String, String> {
                Err("not asked".to_owned())
            }
            fn read_dir(&self, _path: &str) -> Result<Vec<(String, FileType)>, String> {
                Err("not asked".to_owned())
            }
            fn path_exists(&self, _path: &str) -> bool {
                true
            }
            // The disagreeing pair a symlink to a directory produces, so the
            // log says which of the two was asked.
            fn file_type(&self, _path: &str) -> Result<Option<FileType>, String> {
                Ok(Some(FileType::Symlink))
            }
            fn file_type_resolved(&self, _path: &str) -> Result<FileType, String> {
                Ok(FileType::Directory)
            }
        }

        let inner = Dir;
        let host = RecordingHost::new(&inner);
        assert_eq!(
            host.resolve_import("/link-to-dir").ok().as_deref(),
            Some("/link-to-dir/default.nix"),
        );
        assert_eq!(
            host.take().questions(),
            vec![Question::FileTypeResolved("/link-to-dir".to_owned())],
        );
    }

    #[test]
    fn replaying_an_unchanged_world_reproduces_the_key() {
        let inner = fake();
        let host = RecordingHost::new(&inner);
        drop(host.read_file("/a"));
        let recorded = host.take();
        let module = id(b"module");
        assert_eq!(
            recorded.key(&module),
            replayed(&recorded.questions(), &inner).key(&module)
        );
    }

    /// The property the memoisation rests on: a changed answer changes the
    /// key, so the lookup goes somewhere no result was stored.
    #[test]
    fn a_changed_file_changes_the_key() {
        let inner = fake();
        let host = RecordingHost::new(&inner);
        drop(host.read_file("/a"));
        let recorded = host.take();
        let module = id(b"module");
        inner.contents.set(2);
        assert_ne!(
            recorded.key(&module),
            replayed(&recorded.questions(), &inner).key(&module)
        );
    }

    #[test]
    fn a_changed_environment_variable_changes_the_key() {
        struct Env(Cell<bool>);
        impl Host for Env {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                false
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Err("no".to_owned())
            }
            fn get_env(&self, _n: &str) -> Option<String> {
                if self.0.get() {
                    Some("a".to_owned())
                } else {
                    None
                }
            }
        }
        let inner = Env(Cell::new(true));
        let host = RecordingHost::new(&inner);
        drop(host.get_env("V"));
        let recorded = host.take();
        let module = id(b"module");
        inner.0.set(false);
        assert_ne!(
            recorded.key(&module),
            replayed(&recorded.questions(), &inner).key(&module)
        );
    }

    /// Unset and empty are different facts. Digesting them the same way would
    /// serve a result taken with the variable unset to a run where it is set
    /// to the empty string.
    #[test]
    fn an_unset_variable_does_not_digest_as_an_empty_one() {
        struct Unset;
        impl Host for Unset {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                false
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Err("no".to_owned())
            }
            fn get_env(&self, _n: &str) -> Option<String> {
                None
            }
        }
        struct Empty;
        impl Host for Empty {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                false
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Err("no".to_owned())
            }
            fn get_env(&self, _n: &str) -> Option<String> {
                Some(String::new())
            }
        }
        let question = Question::GetEnv("V".to_owned());
        assert_ne!(question.ask(&Unset), question.ask(&Empty));
    }

    /// A read error is part of the answer, not an absence of one: a file that
    /// did not exist and later does must not hit.
    #[test]
    fn a_file_appearing_changes_the_key() {
        let inner = fake();
        let host = RecordingHost::new(&inner);
        drop(host.read_file("/missing"));
        let recorded = host.take();
        let module = id(b"module");

        struct NowPresent;
        impl Host for NowPresent {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                get_env,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Ok("appeared".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                true
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Ok(Some(FileType::Regular))
            }
        }
        assert_ne!(
            recorded.key(&module),
            replayed(&recorded.questions(), &NowPresent).key(&module)
        );
    }

    /// Order is part of the key, because the question sequence is itself a
    /// function of the answers.
    #[test]
    fn question_order_is_part_of_the_key() {
        let inner = fake();
        let module = id(b"module");

        let one = RecordingHost::new(&inner);
        drop(one.read_file("/a"));
        let _ = one.path_exists("/a");
        let first = one.take();

        let other = RecordingHost::new(&inner);
        let _ = other.path_exists("/a");
        drop(other.read_file("/a"));
        let second = other.take();

        assert_ne!(first.key(&module), second.key(&module));
    }

    /// The impurity guard for a coercion rather than a builtin.
    ///
    /// `builtins.purity_tests` enumerates BUILTINS that must go through
    /// `Host`, so it says nothing about `"${/a}"`, which is an op. That
    /// coercion is impure -- its answer is a hash of the file -- and until
    /// ENG-12447 it did not reach `Host` at all, so a read set could not see
    /// it and a memoised result that embedded a store path would have
    /// survived an edit to the file behind it. Drive a real evaluation, not
    /// the trait method, because the thing being asserted is the wiring.
    #[test]
    fn interpolating_a_path_is_a_question_the_read_set_sees() {
        struct WithStore(Cell<u8>);
        impl Host for WithStore {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                get_env,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Err("no".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                true
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Ok(Some(FileType::Regular))
            }
            /// A store path is a hash of the content, so an edit moves it.
            fn copy_to_store(&self, path: &str) -> Result<String, StoreError> {
                Ok(format!(
                    "/nix/store/{}-{}",
                    self.0.get(),
                    path.trim_start_matches('/')
                ))
            }
        }

        let inner = WithStore(Cell::new(1));
        let host = RecordingHost::new(&inner);
        let rendered = match crate::compile::compile_source(
            r#""${/a}""#,
            "/",
            crate::compile::Origin::String,
            &crate::eval::Settings::default(),
        ) {
            Err(e) => format!("compile failed: {e:?}"),
            Ok(module) => {
                let mut vm = crate::vm::Vm::with_settings(crate::eval::Settings::default());
                vm.start_module(&std::rc::Rc::new(module));
                match crate::eval::drive(&mut vm, &host) {
                    Ok(crate::value2::Value::Str(s)) => s.expect_text(),
                    Ok(other) => format!("not a string: {other:?}"),
                    Err(e) => format!("evaluation failed: {e:?}"),
                }
            }
        };
        assert_eq!(rendered, "/nix/store/1-a");

        let recorded = host.take();
        assert_eq!(
            recorded.questions(),
            vec![Question::CopyToStore("/a".to_owned())],
            "the copy did not reach the host, so no read set can see it"
        );

        // And it invalidates: the same file with different content is a
        // different store path, hence a different key.
        let module_hash = id(b"module");
        inner.0.set(2);
        assert_ne!(
            recorded.key(&module_hash),
            replayed(&recorded.questions(), &inner).key(&module_hash)
        );
    }

    /// Two fetches of the same URL are two different questions when they
    /// differ in name, kind or pin, and the key has to say so -- each of the
    /// three changes the store path the answer names.
    ///
    /// Written for the reason the filtered-copy test below was: dropping a
    /// field from [`Question::key_parts`] leaves the codec round trip green,
    /// because that is a different encoding of the same request.
    #[test]
    fn two_fetches_of_one_url_do_not_share_a_key() {
        let request = |name: &str, kind: crate::task::FetchKind, sha: Option<&str>| {
            Question::Fetch(Box::new(crate::task::FetchRequest {
                url: "https://u/x.tar.gz".to_owned(),
                name: name.to_owned(),
                kind,
                expected_sha256: sha.map(str::to_owned),
            }))
        };
        let key = |q: Question| {
            let mut set = ReadSet::default();
            set.entries.push((q, digest(&[b"same-answer"])));
            set.key(&id(b"module"))
        };
        let pin = "sha256-1BdlSaqjNlSVCcgD/PocqAwbnGQ+lyfL6h9WK6+MCJc=";
        let base = key(request(
            "source",
            crate::task::FetchKind::Tarball,
            Some(pin),
        ));
        // A different name: a different store path for identical bytes.
        assert_ne!(
            base,
            key(request("other", crate::task::FetchKind::Tarball, Some(pin)))
        );
        // A different kind: flat ingestion of the tarball rather than the
        // unpacked tree.
        assert_ne!(
            base,
            key(request("source", crate::task::FetchKind::File, Some(pin)))
        );
        // Unpinned. The one that matters most: an unpinned fetch of a URL
        // whose bytes moved must not hit a row recorded when it was pinned.
        assert_ne!(
            base,
            key(request("source", crate::task::FetchKind::Tarball, None))
        );
        // And a different pin.
        assert_ne!(
            base,
            key(request(
                "source",
                crate::task::FetchKind::Tarball,
                Some("sha256-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
            ))
        );
    }

    /// Two filtered copies of the same root under the same name are two
    /// different questions when the filter accepted different files, and the
    /// key has to say so.
    ///
    /// The gap this closes was found by mutation, not by reading: dropping the
    /// accepted list from [`Question::key_parts`] left every test green. The
    /// codec round trip did not catch it because it exercises
    /// [`question_value`], which is a *different* encoding of the same
    /// request -- two encodings, so two things to get wrong.
    #[test]
    fn two_filtered_copies_of_one_root_do_not_share_a_key() {
        let request = |accepted: Option<Vec<crate::task::AcceptedPath>>, name: &str| {
            Question::StoreFiltered(Box::new(crate::task::FilteredCopy {
                root: "/src".to_owned(),
                name: name.to_owned(),
                method: crate::task::PathMethod::NixArchive,
                accepted,
                expected_sha256: None,
                inherit_references: false,
            }))
        };
        let entry = |path: &str| crate::task::AcceptedPath {
            path: path.to_owned(),
            file_type: FileType::Regular,
        };
        let key = |q: Question| {
            let mut set = ReadSet::default();
            set.entries.push((q, digest(&[b"same-answer"])));
            set.key(&id(b"module"))
        };

        let base = key(request(Some(vec![entry("/src/a")]), "src"));
        // A different accepted file: a different tree, a different NAR.
        assert_ne!(base, key(request(Some(vec![entry("/src/b")]), "src")));
        // One more accepted file.
        assert_ne!(
            base,
            key(request(Some(vec![entry("/src/a"), entry("/src/b")]), "src"))
        );
        // A different type for the same path: a symlink and a regular file
        // serialise differently.
        assert_ne!(
            base,
            key(request(
                Some(vec![crate::task::AcceptedPath {
                    path: "/src/a".to_owned(),
                    file_type: FileType::Symlink,
                }]),
                "src"
            ))
        );
        // "accepted nothing" and "no filtering" are different requests.
        assert_ne!(
            key(request(Some(Vec::new()), "src")),
            key(request(None, "src"))
        );
        // And the name is in the key, because it is in the store path.
        assert_ne!(base, key(request(Some(vec![entry("/src/a")]), "other")));
    }

    /// An evaluation that read nothing still has a key, and it is the module's
    /// alone: two different modules that read nothing must not share it.
    #[test]
    fn an_empty_read_set_keys_on_the_module() {
        let empty = ReadSet::default();
        assert_ne!(empty.key(&id(b"one")), empty.key(&id(b"two")));
    }

    // ---- result cache ----------------------------------------------------

    use ix_kernel::cas::MemoryCas;

    fn result(value: &str) -> EvalResult {
        EvalResult {
            status: "ok".to_owned(),
            value: value.to_owned(),
            emissions: Vec::new(),
            token: None,
            pos: None,
        }
    }

    /// Record an evaluation of `/a`, then look it up with nothing changed.
    #[test]
    fn an_unchanged_world_serves_the_memoised_result() -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let module = id(b"module");

        let recorder = RecordingHost::new(&inner);
        drop(recorder.read_file("/a"));
        cache.record(&module, &recorder.take(), &result("v1"))?;

        assert_eq!(
            cache.lookup(&module, &inner, &crate::eval::Settings::default()),
            Some(result("v1"))
        );
        assert_eq!((cache.hits(), cache.wasted_replays()), (1, 0));
        Ok(())
    }

    /// The property the whole design turns on: editing a file the evaluation
    /// read makes the lookup miss, without anything having to notice the edit.
    #[test]
    fn editing_a_file_that_was_read_makes_the_lookup_miss()
    -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let module = id(b"module");

        let recorder = RecordingHost::new(&inner);
        drop(recorder.read_file("/a"));
        cache.record(&module, &recorder.take(), &result("v1"))?;
        assert!(
            cache
                .lookup(&module, &inner, &crate::eval::Settings::default())
                .is_some()
        );

        inner.contents.set(2);
        assert_eq!(
            cache.lookup(&module, &inner, &crate::eval::Settings::default()),
            None,
            "served a pre-edit result"
        );
        Ok(())
    }

    /// A file that was not read is not part of the key, so touching it must
    /// not cost a hit. Over-invalidating is safe but makes the cache useless,
    /// and nothing else in the suite would notice.
    #[test]
    fn editing_a_file_that_was_not_read_still_hits() -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let module = id(b"module");

        let recorder = RecordingHost::new(&inner);
        drop(recorder.get_env("SET"));
        cache.record(&module, &recorder.take(), &result("v1"))?;

        // /a changes; the recorded evaluation never read it.
        inner.contents.set(2);
        assert_eq!(
            cache.lookup(&module, &inner, &crate::eval::Settings::default()),
            Some(result("v1"))
        );
        Ok(())
    }

    /// Two modules that read the same things do not share a row.
    #[test]
    fn the_module_is_part_of_the_key() -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let one = id(b"one");
        let other = id(b"two");

        let recorder = RecordingHost::new(&inner);
        drop(recorder.read_file("/a"));
        cache.record(&one, &recorder.take(), &result("from-one"))?;

        assert_eq!(
            cache.lookup(&one, &inner, &crate::eval::Settings::default()),
            Some(result("from-one"))
        );
        assert_eq!(
            cache.lookup(&other, &inner, &crate::eval::Settings::default()),
            None
        );
        Ok(())
    }

    /// A witness that no longer describes what the evaluation would ask is a
    /// miss, never a wrong answer: the key is built from the answers observed
    /// now, so it addresses a row nothing was ever stored under.
    #[test]
    fn a_stale_witness_misses_rather_than_answering_wrongly()
    -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let module = id(b"module");

        // Recorded reading /a and /missing, in that order.
        let recorder = RecordingHost::new(&inner);
        drop(recorder.read_file("/a"));
        drop(recorder.read_file("/missing"));
        cache.record(&module, &recorder.take(), &result("two-reads"))?;
        assert!(
            cache
                .lookup(&module, &inner, &crate::eval::Settings::default())
                .is_some()
        );

        // A world where the second read now succeeds. The witness still names
        // both files, so replay asks both and gets a different answer for one.
        struct BothPresent;
        impl Host for BothPresent {
            crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
            fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
                self.read_file(path).map(String::into_bytes)
            }
            crate::host::host_stubs!(
                realise,
                store_text,
                write_derivation,
                store_filtered,
                fetch,
                lock_flake,
                fetch_tree,
                not_async,
            );
            crate::host::host_stubs!(
                file_type_resolved,
                get_env,
                copy_to_store,
                ensure_path,
                warn,
                find_file,
                nix_path,
                trace
            );
            fn read_file(&self, _p: &str) -> Result<String, String> {
                Ok("present".to_owned())
            }
            fn read_dir(&self, _p: &str) -> Result<Vec<(String, FileType)>, String> {
                Ok(Vec::new())
            }
            fn path_exists(&self, _p: &str) -> bool {
                true
            }
            fn file_type(&self, _p: &str) -> Result<Option<FileType>, String> {
                Ok(Some(FileType::Regular))
            }
        }
        assert_eq!(
            cache.lookup(&module, &BothPresent, &crate::eval::Settings::default()),
            None
        );
        Ok(())
    }

    /// Recording the same evaluation twice is idempotent: one row, and the
    /// second record does not invent a second answer for the same key.
    #[test]
    fn recording_twice_keeps_one_row() -> Result<(), Box<dyn core::error::Error>> {
        let inner = fake();
        let cas = MemoryCas::new();
        let mut cache = ResultCache::new(&cas);
        let module = id(b"module");

        for _ in 0..2 {
            let recorder = RecordingHost::new(&inner);
            drop(recorder.read_file("/a"));
            cache.record(&module, &recorder.take(), &result("v1"))?;
        }
        assert_eq!(
            cache.lookup(&module, &inner, &crate::eval::Settings::default()),
            Some(result("v1"))
        );
        Ok(())
    }
}

/// The asynchronous route to a host records what the blocking route records.
#[cfg(test)]
mod begun_questions {
    use super::{ReadSet, RecordingHost};
    use crate::host::{FileType, Host, SlowAnswer, StoreError, Ticket};
    use crate::task::{FetchKind, FetchRequest};

    /// A host that answers a fetch, and can do it either way.
    ///
    /// The asynchronous half is not threaded: what is under test is what the
    /// recorder writes down, and a thread would only add a way for the test
    /// to be flaky. `begin` computes the answer at once and `collect` hands
    /// it over, which is a legal host -- `begin` promises not to block, not
    /// to be slow.
    #[derive(Default)]
    struct Both {
        answered: std::cell::RefCell<std::collections::HashMap<u64, String>>,
        asynchronous: bool,
    }

    impl Host for Both {
        crate::host::host_stubs!(parse_flake_ref, flake_ref_to_string);
        fn read_file_bytes(&self, path: &str) -> Result<Vec<u8>, String> {
            self.read_file(path).map(String::into_bytes)
        }
        crate::host::host_stubs!(
            realise,
            store_text,
            write_derivation,
            store_filtered,
            lock_flake,
            fetch_tree,
        );
        crate::host::host_stubs!(
            get_env,
            copy_to_store,
            ensure_path,
            find_file,
            nix_path,
            trace,
            warn,
            file_type_resolved
        );
        fn read_file(&self, p: &str) -> Result<String, String> {
            Err(format!("path '{p}' does not exist"))
        }
        fn read_dir(&self, p: &str) -> Result<Vec<(String, FileType)>, String> {
            Err(format!("path '{p}' does not exist"))
        }
        fn path_exists(&self, _p: &str) -> bool {
            false
        }
        fn file_type(&self, p: &str) -> Result<Option<FileType>, String> {
            Err(format!("path '{p}' does not exist"))
        }
        fn fetch(&self, request: &FetchRequest) -> Result<String, StoreError> {
            Ok(format!(
                "/nix/store/0000000000000000000000000000000a-{}",
                request.name
            ))
        }
        fn begin(&self, question: &crate::host::Slow<'_>) -> Option<Ticket> {
            if !self.asynchronous {
                return None;
            }
            let crate::host::Slow::Fetch(request) = question else {
                return None;
            };
            let answer = self.fetch(request).ok()?;
            let ticket = self.answered.borrow().len() as u64 + 1;
            self.answered.borrow_mut().insert(ticket, answer);
            Some(Ticket(ticket))
        }
        fn collect(&self, ticket: Ticket, _block: bool) -> Option<SlowAnswer> {
            let answer = self.answered.borrow_mut().remove(&ticket.0)?;
            Some(SlowAnswer::Store(Ok(answer)))
        }
    }

    fn request() -> FetchRequest {
        FetchRequest {
            url: "http://example.invalid/x".to_owned(),
            name: "x".to_owned(),
            kind: FetchKind::File,
            expected_sha256: None,
        }
    }

    /// The recorder logs the same question and the same answer digest either
    /// way round.
    ///
    /// This is what makes the asynchronous path invisible to the memo. A
    /// read set recorded through `begin`/`collect` has to key identically to
    /// one recorded through `fetch`, or a witness written by an embedder with
    /// an asynchronous host would never match one written by an embedder
    /// without -- two caches for one evaluation, and neither would say so.
    #[test]
    fn a_begun_question_records_as_the_blocking_one() -> Result<(), String> {
        let blocking = Both::default();
        let recorder = RecordingHost::new(&blocking);
        let _ = recorder.fetch(&request());
        let by_blocking: ReadSet = recorder.take();

        let asynchronous = Both {
            asynchronous: true,
            ..Both::default()
        };
        let recorder = RecordingHost::new(&asynchronous);
        let question = crate::host::Slow::Fetch(&request());
        let ticket = recorder
            .begin(&question)
            .ok_or_else(|| "the host declined to begin a fetch".to_owned())?;
        let _ = recorder
            .collect(ticket, true)
            .ok_or_else(|| "the host did not answer a ticket it minted".to_owned())?;
        let by_beginning: ReadSet = recorder.take();

        if by_blocking != by_beginning {
            return Err(format!(
                "the two routes recorded differently: blocking {by_blocking:?}, \
                 begun {by_beginning:?}"
            ));
        }
        Ok(())
    }

    /// Nothing is recorded until the answer exists.
    ///
    /// A question noted at `begin` would have to invent an answer digest, and
    /// a read set whose answer does not match what the evaluation was told is
    /// the one thing this file exists to prevent.
    #[test]
    fn beginning_a_question_records_nothing_on_its_own() -> Result<(), String> {
        let host = Both {
            asynchronous: true,
            ..Both::default()
        };
        let recorder = RecordingHost::new(&host);
        let question = crate::host::Slow::Fetch(&request());
        let _ = recorder
            .begin(&question)
            .ok_or_else(|| "the host declined to begin a fetch".to_owned())?;
        let so_far = recorder.take();
        if !so_far.questions().is_empty() {
            return Err(format!("a begun question was already recorded: {so_far:?}"));
        }
        Ok(())
    }
}
