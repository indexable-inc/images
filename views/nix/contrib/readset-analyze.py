#!/usr/bin/env python3
"""Analyse read-set traces produced by nix's read-set-trace-file setting.

One trace: reports what the evaluation read, grouped by the tree each input
came from, and where the first read of a chosen tree happened in time.

Two traces: reports what share of the attributable evaluation time sits in
tracked entries whose read set is not byte-identical between them, and then
what share is left once that verdict is carried along the edges between
entries. The second number is the one that matters: 80,285 of the 91,758
entries in the traced host read no files at all, so a read set alone cannot
show them either valid or invalid, and their values arrived from entries that
can be decided.

The script fails rather than reporting zeros when a trace has no entries or
no inputs, because a trace of an evaluation that recorded nothing looks
exactly like a trace of an evaluation that read nothing.
"""

import argparse
import json
import sys
from collections import defaultdict


class Trace:
    def __init__(self, path):
        self.path = path
        self.inputs = {}    # id -> dict(kind, rel, path, tree, first_ns)
        self.observed = {}  # id -> what the read observed
        self.trees = {}     # id -> dict(root, display, fp): which tree, and its version
        self.entries = []
        self.summary = None
        self.summary_count = 0
        with open(path) as f:
            for line in f:
                line = line.strip()
                if not line:
                    continue
                r = json.loads(line)
                t = r["t"]
                if t == "in":
                    self.inputs[r["id"]] = r
                elif t == "obs":
                    self.observed[r["id"]] = r["v"]
                elif t == "tree":
                    self.trees[r["id"]] = r
                elif t == "entry":
                    self.entries.append(r)
                elif t == "summary":
                    self.summary = r
                    self.summary_count += 1
        self.check_intact()

    def check_intact(self):
        """Refuse a trace that is not the whole of exactly one evaluation.

        The summary is written last, so its absence means the evaluator died,
        and its counts are declared independently of the records themselves, so
        comparing the two catches a file that was truncated or left over from an
        earlier run. Both failures otherwise present as a well formed trace that
        simply describes less work, which reads as a real result: an evaluation
        that crashed early and one that had little to do are indistinguishable
        from the shape of the file alone.
        """
        if self.summary is None:
            die(f"{self.path}: no summary record, so the evaluator did not shut "
                f"down cleanly and this trace is a fragment")
        if self.summary_count != 1:
            die(f"{self.path}: {self.summary_count} summary records, so this file "
                f"holds more than one evaluation")
        for name, declared, actual in (
            ("entries", self.summary["entries"], len(self.entries)),
            ("inputs", self.summary["inputs"], len(self.inputs)),
        ):
            if declared != actual:
                die(f"{self.path}: the summary declares {declared} {name} but the "
                    f"file holds {actual}, so it is truncated or interleaved")

    def check_not_empty(self):
        """A trace with no entries or no inputs is a broken hook, not a quiet evaluation."""
        problems = []
        if not self.entries:
            problems.append("no tracked entries")
        if not self.inputs:
            problems.append("no recorded inputs")
        if not any(e["kind"] == "root" for e in self.entries):
            problems.append("no entry of kind root, so the trace is truncated")
        # An expression with no imports and no derivations is legitimate, but a
        # trace of a real evaluation with neither is a hook that did not fire.
        if not any(e["kind"] in ("import", "derivation") for e in self.entries):
            problems.append("no entries of kind import or derivation")
        n_empty = sum(1 for e in self.entries if not e["inputs"])
        if n_empty == len(self.entries):
            problems.append("every entry has an empty read set")
        # A trace with no edges is a graph with nothing to propagate along,
        # which reads in every downstream number as an evaluation where
        # nothing depended on anything. That is what a deleted recording
        # looks like, so refuse it here rather than reporting the phase 1
        # answer under a phase 2 heading.
        if not any(e.get("edges") for e in self.entries):
            problems.append("no entry records any edge to another entry")
        if problems:
            die(f"{self.path}: " + "; ".join(problems))

    def signature(self, entry, key="abs", tree_map=None):
        """The read set as a comparable value.

        Four keyings, because the gap between them is the result. `tree` is
        the model as the design states it: which tree, where in that tree, and
        what was observed. `rel` is the same without the tree, which brackets
        the model from the other side: a tree id is assigned on first sight
        within one run, so two runs number the same tree differently as soon
        as anything shifts the order, and on the measured pair that renamed
        16,905 of 53,616 inputs whose bytes were identical. Dropping the tree
        cannot tell two trees that both hold `/flake.nix` apart, so `rel` is a
        lower bound and `tree` an upper one until a tree carries a name that
        survives a run.
        `abs` is what a cache keyed on absolute paths sees, which for anything
        inside a flake source means a path containing the hash of the whole
        tree. `fingerprint` is what a cache keyed on the accessor's own
        fingerprint sees, which is the same defect by another route because
        for a clean git tree that fingerprint is the revision.
        """
        out = []
        for i in entry["inputs"]:
            inp = self.inputs.get(i)
            if inp is None:
                out.append(("<unknown input id>", str(i), ""))
                continue
            value = self.observed.get(i, "")
            # An input with no tree is one no accessor answered for: a tree
            # attribute or a store query. It is in no tree in either run, so it
            # names itself the same way on both sides and must not be sent
            # through the pairing, which would give it one name here and
            # another there and read as 298 entries having changed.
            tree_id = inp.get("tree")
            tree = self.trees.get(tree_id, {}) if tree_id is not None else {}
            if tree_id is None:
                tree_id = "«no tree»"
            elif tree_map is not None:
                tree_id = tree_map.get(tree_id, f"unpaired {tree_id}")
            if key == "abs":
                # Named by absolute path, which for anything inside a flake
                # source contains the hash of the whole tree. This is what a
                # cache keyed on paths sees, and it is why every input under an
                # edited tree reads as a different input.
                out.append((inp["kind"], inp["path"], value))
            elif key == "fingerprint":
                # Named by the accessor's own fingerprint, which looks like the
                # sound choice and is the same defect by another route: for a
                # clean git tree that fingerprint is the revision.
                out.append((inp["kind"], inp["rel"], tree.get("fp", ""), value))
            elif key == "rel":
                # Where in whichever tree answered, and what was observed. Not
                # sound on its own; see the note above.
                out.append((inp["kind"], inp["rel"], value))
            else:
                # The model: which tree, where in it, and what was observed.
                out.append((inp["kind"], str(tree_id), inp["rel"], value))
        return tuple(sorted(out))

    def key(self, entry):
        # Deliberately not the accessor. An accessor number is a process-local
        # counter assigned in creation order, so it has no meaning across two
        # runs. Two trees that both hold `/flake.nix` are separated instead by
        # the occurrence-index pairing in `report_compare`, which is what that
        # pairing is for.
        #
        # This does not make an import key stable across an edit. An entry's key
        # is its path within the answering accessor, and for the store accessor
        # that path is the full store path, so an import resolved through the
        # store is named by a path containing the hash of the whole tree. In one
        # measured pair 664 of 5,732 import keys moved for that reason, all of
        # them under one store path, holding 0.061s of 20.7s. That is the same
        # whole-tree naming that changes the read sets, showing up as key churn
        # instead.
        return (entry["kind"], entry["key"])


def pair_trees(a, b):
    """Map each tree id in `b` to the id of the same tree in `a`.

    A tree id is a counter assigned on first sight within one run, so it means
    nothing across two. What does mean something is the `(identity, view)` the
    trace now records: the fingerprint with its version component removed, or
    failing that the accessor's own display, plus which of the up-to-three
    views of one tree this record is.

    There is no fallback. An earlier version paired whatever was left over by
    arrival order, which silently mapped one run's store-path view onto the
    other's filesystem-root view and reported 99.2% of the evaluation
    invalidated where the answer is 4.3%. A pairing that cannot be justified is
    refused rather than guessed, because the guess is indistinguishable from a
    result.
    """
    anon = [t for t in list(a.trees.values()) + list(b.trees.values()) if t.get("anonymous")]
    if anon:
        die(f"{b.path}: {len(anon)} tree records carry no identity "
            f"(accessors {sorted({t.get('acc') for t in anon})}), so they can only be "
            f"paired by position; refusing rather than reporting a number that "
            f"depends on the order two runs happened to see their trees in")
    missing = [t for t in list(a.trees.values()) + list(b.trees.values()) if "identity" not in t]
    if missing:
        die(f"{b.path}: {len(missing)} tree records have no identity field at all, so this "
            f"trace predates the identity recording and cannot be compared soundly")

    def key(t):
        return (t["identity"], t.get("view", ""))

    counts = {"identity": 0, "unpaired-b": 0, "unmatched-a": 0}
    idx = {}
    for t in sorted(a.trees.values(), key=lambda t: t["id"]):
        idx.setdefault(key(t), []).append(t["id"])

    out, taken = {}, set()
    for t in sorted(b.trees.values(), key=lambda t: t["id"]):
        cands = [i for i in idx.get(key(t), []) if i not in taken]
        if cands:
            out[t["id"]] = cands[0]
            taken.add(cands[0])
            counts["identity"] += 1
        else:
            counts["unpaired-b"] += 1
    counts["unmatched-a"] = len(a.trees) - len(taken)
    return out, counts


def die(msg):
    print(f"error: {msg}", file=sys.stderr)
    sys.exit(1)


def root_of(inp, trees=None):
    """The tree an input belongs to.

    The evaluator now records this: an input carries the id of the tree that
    answered for it, and each tree's root is written down once. Falling back to
    slicing the path only matters for a trace taken before that.
    """
    if trees is not None and "tree" in inp:
        t = trees.get(inp["tree"])
        if t is not None:
            return t.get("root") or f"tree {inp['tree']}"
    p = inp.get("path", "")
    if p.startswith("/nix/store/"):
        return "/".join(p.split("/")[:4])
    parts = [x for x in p.split("/") if x]
    return "/" + "/".join(parts[:2])


def fmt_ns(ns):
    return f"{ns / 1e9:.3f}s"


def report_one(tr, tree_prefix):
    s = tr.summary
    print(f"== {tr.path}")
    print(f"   wall {fmt_ns(s['wall_ns'])}, thread cpu {fmt_ns(s['cpu_ns'])}")
    print(f"   entries {s['entries']}, distinct inputs {s['inputs']}, recorded reads {s['reads']}")
    for k in ("root", "import", "derivation", "option"):
        print(f"   entries of kind {k}: {s.get('kind_' + k, 0)}")

    n_edges = sum(len(e.get("edges", ())) for e in tr.entries)
    with_edges = sum(1 for e in tr.entries if e.get("edges"))
    print(f"   edges {n_edges} over {with_edges} entries "
          f"({100.0 * with_edges / len(tr.entries) if tr.entries else 0:.1f}% of entries "
          f"record at least one)")
    print("   edges by how they were observed (recorded, before deduplication): "
          + ", ".join(f"{k} {s.get('edge_' + k, 0)}"
                      for k in ("demand", "reuse", "derivation")))

    total_excl = sum(e["cpu_excl_ns"] for e in tr.entries)
    total_wall_excl = sum(e["wall_excl_ns"] for e in tr.entries)
    print(f"   sum of per-entry exclusive thread cpu: {fmt_ns(total_excl)}")
    print(f"   sum of per-entry exclusive wall:       {fmt_ns(total_wall_excl)} "
          f"of {fmt_ns(s['wall_ns'])} total wall")

    # Which tree each entry read, weighted by the time attributed to the entry.
    per_root_entries = defaultdict(int)
    per_root_cpu = defaultdict(int)
    per_root_inputs = defaultdict(int)
    for inp in tr.inputs.values():
        per_root_inputs[root_of(inp, tr.trees)] += 1
    for e in tr.entries:
        roots = {root_of(tr.inputs[i], tr.trees) for i in e["inputs"] if i in tr.inputs}
        for r in roots:
            per_root_entries[r] += 1
            per_root_cpu[r] += e["cpu_excl_ns"]

    # An entry that read no files is not an entry that depends on nothing: its
    # value came from its caller. A read set over file inputs cannot decide
    # whether such an entry is still valid, so its time is reported apart from
    # the rest rather than counted as reusable.
    empty_n = sum(1 for e in tr.entries if not e["inputs"])
    empty_cpu = sum(e["cpu_excl_ns"] for e in tr.entries if not e["inputs"])
    print(f"   entries with an empty read set: {empty_n} of {len(tr.entries)} "
          f"({100.0 * empty_n / len(tr.entries):.1f}%), holding {fmt_ns(empty_cpu)} "
          f"of {fmt_ns(total_excl)} attributed cpu "
          f"({100.0 * empty_cpu / total_excl if total_excl else 0:.1f}%)")
    for kind in ("import", "derivation", "option"):
        ke = [e for e in tr.entries if e["kind"] == kind]
        if not ke:
            continue
        kempty = sum(1 for e in ke if not e["inputs"])
        print(f"     of kind {kind}: {kempty} of {len(ke)} have an empty read set")
    print("   read-set fan-out by tree (entries reading it, share of entries, exclusive cpu):")
    for r, n in sorted(per_root_entries.items(), key=lambda kv: -kv[1])[:15]:
        share = 100.0 * n / len(tr.entries)
        print(f"     {n:7d} ({share:5.1f}%)  {fmt_ns(per_root_cpu[r]):>9}  "
              f"{per_root_inputs[r]:6d} inputs  {r}")

    if tree_prefix:
        matching = [i for i in tr.inputs.values() if i["path"].startswith(tree_prefix)]
        print(f"   inputs under {tree_prefix}: {len(matching)} of {len(tr.inputs)}")
        if matching:
            first = min(matching, key=lambda i: i["first_ns"])
            print(f"   first read of that tree at {fmt_ns(first['first_ns'])} "
                  f"({100.0 * first['first_ns'] / s['wall_ns']:.1f}% of total wall): {first['path']}")
            ids = {i["id"] for i in matching}
            n = sum(1 for e in tr.entries if ids & set(e["inputs"]))
            cpu = sum(e["cpu_excl_ns"] for e in tr.entries if ids & set(e["inputs"]))
            print(f"   entries reading that tree: {n} of {len(tr.entries)} "
                  f"({100.0 * n / len(tr.entries):.1f}%), exclusive cpu {fmt_ns(cpu)} "
                  f"({100.0 * cpu / total_excl if total_excl else 0:.1f}% of attributed cpu)")
        else:
            print("   WARNING: no inputs matched that prefix. Check the prefix before "
                  "reading this as a result.")
    print()


def report_compare(a, b, key):
    print(f"== comparing {a.path} (baseline) with {b.path} (after edit), inputs keyed on {key}")

    tree_map, tree_counts = pair_trees(a, b)
    if key == "tree":
        print("   trees paired between the two runs: "
              + ", ".join(f"{k} {v}" for k, v in tree_counts.items()))
        # A record with no partner is a tree one run saw and the other did not,
        # which under lazy trees is materialisation firing at different points.
        # Reported rather than absorbed, because it is the difference between
        # the two runs having done the same work and not.

    # An entry key is (kind, name), and names repeat: one derivation name occurs
    # once per instantiation. Pair the nth occurrence of a key in one trace with
    # the nth in the other, in entry order, so that every entry is compared
    # rather than only the first of each name. Occurrences with no partner are
    # counted as new or gone, which is the honest answer for a key whose
    # multiplicity moved.
    by_key_a = defaultdict(list)
    for e in a.entries:
        by_key_a[a.key(e)].append(e)
    by_key_b = defaultdict(list)
    for e in b.entries:
        by_key_b[b.key(e)].append(e)

    keys_a, keys_b = set(by_key_a), set(by_key_b)
    common = keys_a & keys_b
    print(f"   entries: {len(a.entries)} baseline, {len(b.entries)} after")
    print(f"   entry keys: {len(keys_a)} baseline, {len(keys_b)} after, "
          f"{len(common)} in both, {len(keys_a - keys_b)} gone, {len(keys_b - keys_a)} new")
    dup_a = sum(1 for k in keys_a if len(by_key_a[k]) > 1)
    print(f"   keys occurring more than once in the baseline: {dup_a}; "
          f"most occurrences of one key: {max(len(v) for v in by_key_a.values())}")

    total_cpu_b = sum(e["cpu_excl_ns"] for e in b.entries)
    if total_cpu_b == 0:
        die("the second trace attributes no cpu time to any entry")

    changed_cpu = same_cpu = new_cpu = 0
    changed_n = same_n = new_n = 0
    # Ids rather than counts, because these seed the propagation below.
    seed_ids = set()
    # A derivation records its store path, which is a hash of everything that
    # went into it. Whether that path moved is the ground truth the read sets
    # and the edges are both trying to predict, and it is the only thing in a
    # trace that says an entry's answer changed rather than that its inputs
    # were touched.
    drv_moved, drv_same = set(), set()
    empty_same_cpu = 0
    empty_same_n = 0
    changed_by_key = defaultdict(int)
    by_kind = defaultdict(lambda: defaultdict(int))
    # Which tree the inputs that actually differ belong to. This is what says
    # whether an entry was invalidated by the file that was edited or by a
    # whole-tree dependency that names the edit only through a store path.
    cause_entries = defaultdict(int)
    cause_cpu = defaultdict(int)
    for k, group_b in by_key_b.items():
        group_a = by_key_a.get(k, [])
        for i, e in enumerate(group_b):
            if i >= len(group_a):
                new_cpu += e["cpu_excl_ns"]
                new_n += 1
                seed_ids.add(e["id"])
                by_kind[e["kind"]]["new_n"] += 1
                by_kind[e["kind"]]["new_cpu"] += e["cpu_excl_ns"]
                continue
            produced_a = group_a[i].get("produced")
            produced_b = e.get("produced")
            if produced_a is not None and produced_b is not None:
                (drv_moved if produced_a != produced_b else drv_same).add(e["id"])
            sig_a = a.signature(group_a[i], key)
            sig_b = b.signature(e, key, tree_map if key == "tree" else None)
            if sig_a == sig_b:
                same_cpu += e["cpu_excl_ns"]
                same_n += 1
                by_kind[e["kind"]]["same_n"] += 1
                by_kind[e["kind"]]["same_cpu"] += e["cpu_excl_ns"]
                if not e["inputs"]:
                    empty_same_cpu += e["cpu_excl_ns"]
                    empty_same_n += 1
            else:
                changed_cpu += e["cpu_excl_ns"]
                changed_n += 1
                seed_ids.add(e["id"])
                changed_by_key[k] += e["cpu_excl_ns"]
                by_kind[e["kind"]]["changed_n"] += 1
                by_kind[e["kind"]]["changed_cpu"] += e["cpu_excl_ns"]
                diff = set(sig_b) - set(sig_a)
                roots = set()
                for item in diff:
                    if key == "tree":
                        # The ids in a `tree`-keyed signature are `a`'s, since
                        # that is the namespace `pair_trees` maps into.
                        t = a.trees.get(int(item[1])) if item[1].lstrip("-").isdigit() else None
                        roots.add((t.get("root") if t else None) or f"tree {item[1]}")
                    else:
                        roots.add(root_of({"path": item[1]}))
                if not roots:
                    roots = {"<inputs removed only>"}
                for r in roots:
                    cause_entries[r] += 1
                    cause_cpu[r] += e["cpu_excl_ns"]

    covered = same_n + changed_n + new_n
    print(f"   entries compared: {covered} of {len(b.entries)} "
          f"({100.0 * covered / len(b.entries):.1f}%), covering "
          f"{fmt_ns(same_cpu + changed_cpu + new_cpu)} of {fmt_ns(total_cpu_b)} "
          f"attributed cpu "
          f"({100.0 * (same_cpu + changed_cpu + new_cpu) / total_cpu_b:.1f}%)")
    changed_examples = [(v, k) for k, v in changed_by_key.items()]
    changed_examples.sort(reverse=True)
    changed_examples = changed_examples[:10]

    print(f"   read set unchanged: {same_n} entries, {fmt_ns(same_cpu)} exclusive cpu "
          f"({100.0 * same_cpu / total_cpu_b:.1f}%)")
    print(f"   read set changed:   {changed_n} entries, {fmt_ns(changed_cpu)} exclusive cpu "
          f"({100.0 * changed_cpu / total_cpu_b:.1f}%)")
    print(f"   entry is new:       {new_n} entries, {fmt_ns(new_cpu)} exclusive cpu "
          f"({100.0 * new_cpu / total_cpu_b:.1f}%)")
    print(f"   invalidated share (changed plus new) of attributed exclusive cpu: "
          f"{100.0 * (changed_cpu + new_cpu) / total_cpu_b:.1f}% "
          f"of {fmt_ns(total_cpu_b)}")
    print(f"   of the unchanged, {empty_same_n} entries holding {fmt_ns(empty_same_cpu)} "
          f"({100.0 * empty_same_cpu / total_cpu_b:.1f}%) read no files at all, so "
          f"file inputs cannot show them either valid or invalid")
    reusable = same_cpu - empty_same_cpu
    print(f"   reusable on the evidence of file inputs alone: {fmt_ns(reusable)} "
          f"({100.0 * reusable / total_cpu_b:.1f}%); everything else is "
          f"{100.0 * (total_cpu_b - reusable) / total_cpu_b:.1f}%")
    report_propagation(b, seed_ids, total_cpu_b, changed_cpu + new_cpu, drv_moved, drv_same)
    print(f"   for scale, total eval wall was {fmt_ns(b.summary['wall_ns'])} and the "
          f"entries account for {fmt_ns(total_cpu_b)} of thread cpu")
    print("   by entry kind (unchanged / changed / new, entries and exclusive cpu):")
    for kind, d in sorted(by_kind.items()):
        print(f"     {kind:11} {d['same_n']:7d} {fmt_ns(d['same_cpu']):>9} | "
              f"{d['changed_n']:7d} {fmt_ns(d['changed_cpu']):>9} | "
              f"{d['new_n']:7d} {fmt_ns(d['new_cpu']):>9}")
    print("   what the changed entries read that differs (tree, entries, their exclusive cpu):")
    for r, n in sorted(cause_entries.items(), key=lambda kv: -cause_cpu[kv[0]])[:10]:
        print(f"     {n:7d} {fmt_ns(cause_cpu[r]):>9}  {r}")
    print("   largest entries whose read set changed:")
    for cpu, k in sorted(changed_examples, reverse=True):
        print(f"     {fmt_ns(cpu):>9}  {k[0]}: {k[1][:110]}")
    print()


def propagate(entries, seed_ids):
    """Close a set of directly invalidated entries under the edges.

    An entry's `edges` name the entries whose values flowed into it, so
    invalidation travels the other way: from a producer to everything that
    demanded it. The graph can hold cycles, because a boundary re-entered
    during its own evaluation demands an entry that is still open, so this is
    a worklist over a visited set rather than a recursion.
    """
    consumers = defaultdict(list)
    for e in entries:
        for producer in e.get("edges", ()):
            consumers[producer].append(e["id"])

    invalid = set(seed_ids)
    work = list(seed_ids)
    while work:
        producer = work.pop()
        for consumer in consumers.get(producer, ()):
            if consumer not in invalid:
                invalid.add(consumer)
                work.append(consumer)
    return invalid


def report_propagation(b, seed_ids, total_cpu, direct_cpu, drv_moved=frozenset(), drv_same=frozenset()):
    """What the edges add to the verdict the read sets alone reached."""
    by_id = {e["id"]: e for e in b.entries}
    n_edges = sum(len(e.get("edges", ())) for e in b.entries)
    if n_edges == 0:
        die(f"{b.path}: no edges, so propagation has nothing to walk and the "
            f"number below would be the read-set answer under another name")

    invalid = propagate(b.entries, seed_ids)
    invalid_cpu = sum(by_id[i]["cpu_excl_ns"] for i in invalid if i in by_id)
    added = invalid - set(seed_ids)
    added_cpu = sum(by_id[i]["cpu_excl_ns"] for i in added if i in by_id)

    # What an entry reached only through edges looks like: it read nothing that
    # changed, and in most cases read nothing at all, which is exactly the
    # class a read set cannot decide.
    added_empty = sum(1 for i in added if i in by_id and not by_id[i]["inputs"])

    print(f"   edges in the after trace: {n_edges} over "
          f"{sum(1 for e in b.entries if e.get('edges'))} entries")
    print(f"   invalidated directly (read set changed or entry is new): "
          f"{len(seed_ids)} entries, {fmt_ns(direct_cpu)} "
          f"({100.0 * direct_cpu / total_cpu:.1f}%)")
    print(f"   reached only along edges: {len(added)} entries ({added_empty} of them "
          f"read no files at all), {fmt_ns(added_cpu)} "
          f"({100.0 * added_cpu / total_cpu:.1f}%)")
    print(f"   invalidated with edge propagation: {len(invalid)} of {len(b.entries)} "
          f"entries, {fmt_ns(invalid_cpu)} of {fmt_ns(total_cpu)} "
          f"({100.0 * invalid_cpu / total_cpu:.1f}%)")
    by_kind_added = defaultdict(lambda: [0, 0])
    for i in added:
        e = by_id.get(i)
        if e is None:
            continue
        by_kind_added[e["kind"]][0] += 1
        by_kind_added[e["kind"]][1] += e["cpu_excl_ns"]
    if by_kind_added:
        print("   entries the edges added, by kind:")
        for kind, (n, cpu) in sorted(by_kind_added.items()):
            print(f"     {kind:11} {n:7d} {fmt_ns(cpu):>9}")

    if not drv_moved and not drv_same:
        print("   no entry records a produced value, so nothing here can be scored "
              "against what actually moved")
        return
    # Recall is the number that decides whether a cache built on this is sound:
    # a derivation that moved and was not reached is a stale answer served.
    # The false positives are only wasted work.
    print(f"   derivations paired between the two runs: {len(drv_moved) + len(drv_same)}, "
          f"of which {len(drv_moved)} produced a different store path")
    for label, flagged in (("read sets alone", set(seed_ids)), ("with edge propagation", invalid)):
        hit = len(drv_moved & flagged)
        wrong = len(drv_same & flagged)
        print(f"     {label:22} reaches {hit} of {len(drv_moved)} moved"
              f"{f' ({100.0 * hit / len(drv_moved):.1f}%)' if drv_moved else ''}"
              f", and {wrong} of {len(drv_same)} that did not move")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("trace", nargs="+")
    ap.add_argument("--tree-prefix", default=None,
                    help="path prefix of the tree under edit, for the fan-out and "
                         "fork-prefix numbers")
    ap.add_argument("--compare", action="store_true",
                    help="treat the first two traces as baseline and after-edit")
    ap.add_argument("--allow-empty", action="store_true",
                    help="do not fail on a trace with no entries; only for testing the check")
    args = ap.parse_args()

    traces = [Trace(p) for p in args.trace]
    if not args.allow_empty:
        for tr in traces:
            tr.check_not_empty()
    for tr in traces:
        report_one(tr, args.tree_prefix)
    if args.compare:
        if len(traces) < 2:
            die("--compare needs two traces")
        for key in ("tree", "rel", "abs", "fingerprint"):
            report_compare(traces[0], traces[1], key)


if __name__ == "__main__":
    main()
