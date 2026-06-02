"""Normalize Claude Code history into polars frames + the aggregates /optimize reasons over.

No third-party deps beyond polars. Designed to run in the index MCP python session
(polars preinstalled), so the heavy 800 MB scan stays out of the model's context and
only compact aggregates come back. Interactive:

    import sys; sys.path.insert(0, "<CLAUDE_SKILL_DIR>/assets")
    import build_history_df as o
    F = o.build_frames(days=45)        # or full=True
    o.report(F)                         # print the leaderboards
    o.write_html(F, "~/.claude/optimize/report.html")   # browser report
    F["df"]; F["bash"]; F["tools"]; F["debug"]   # slice further with polars

Standalone / headless (the `optimize-scan` portable service runs this via uv):
    python build_history_df.py --days 60 --out ~/.claude/optimize
writes parquet caches (incl. history_debug.parquet), findings.json, and report.html.

Schema verified against real transcripts:
  - usage.speed is a categorical label ("fast"), NOT tokens/sec
  - usage.iterations is treated as a list of per-iteration usage structs (count = len, else 0)
  - per-command wall-clock = tool_result.ts - tool_use.ts, matched by tool_use_id
  - a REAL human prompt is a string-content user entry with a top-level promptId;
    harness-injected user messages have no promptId and must be excluded
  - "tool too big / context trimmed" is detected by RESULT SIZE, not a marker string

Debug logs (~/.claude/debug/<session>.txt) verified against real --debug runs:
  - written ONLY for sessions launched with --debug (the `cld` alias); plain
    `claude` writes nothing, so F["debug"] is SPARSE (a few sessions, not all)
  - same cleanupPeriodDays retention as transcripts deletes old files
  - filename stem == session id, so it joins to the transcript `session` column
  - carries timing the transcript cannot: per-tool auto-mode classifier latency,
    permission-decision + dispatch latency, and API time-to-first-byte
"""
from __future__ import annotations
import argparse, glob, html, json, os, re, sys
from datetime import datetime, timezone

PROJECTS = os.path.expanduser("~/.claude/projects")
CORRECTION = re.compile(
    r"\b(no|nope|wrong|don'?t|stop|wait|actually|revert|undo|that'?s not|not what i|"
    r"you (broke|deleted|removed|changed)|why did you|i (said|asked)|incorrect|misread)\b",
    re.I,
)
HARNESS_PREFIXES = ("<task-notification", "<system-reminder", "<local-command",
                    "[request interrupted", "caveat:", "<command-",
                    "a session-scoped", "this session is being continued",
                    "the user opened", "the user's message")

# Escape hatches / jank: workaround patterns in commands. A pattern recurring
# across many sessions is a candidate for a proper architecture fix (the thing
# the workaround compensates for). Signal 7.
JANK = [
    ("silenced stderr (2>/dev/null)", re.compile(r"2>\s*/dev/null")),
    ("swallowed failure (|| true)", re.compile(r"\|\|\s*(true|:)\b")),
    ("skipped hooks (--no-verify)", re.compile(r"--no-verify\b")),
    ("force git (--force/-f)", re.compile(r"\b(push|commit|checkout)\b[^|;&]*(--force(-with-lease)?\b|\s-f\b)")),
    ("sleep timing hack", re.compile(r"\bsleep\s+\d")),
    ("retry/poll loop (while !)", re.compile(r"\bwhile\s*!")),
    ("insecure TLS (curl -k/--insecure)", re.compile(r"\bcurl\b[^|;&]*(\s-k\b|--insecure)")),
    ("nix --impure", re.compile(r"--impure\b")),
    ("flake override (--override-input)", re.compile(r"--override-input\b")),
    ("in-place patch (sed -i)", re.compile(r"\bsed\s+-i\b")),
    ("chmod 777", re.compile(r"\bchmod\s+(-R\s+)?777\b")),
    ("sandbox bypass", re.compile(r"(--no-sandbox|--dangerously|dangerouslyDisable)")),
    ("disable/skip toggle", re.compile(r"\b([A-Z_]*(DISABLE|SKIP)[A-Z_]*)=")),
]
TRUNC = re.compile(r"\[truncated\]")
# Self-correction / backtracking markers inside VISIBLE thinking text. A turn
# whose reasoning contains these is mid-flight course-correcting (thrash). Only
# meaningful where thinking text exists (display=summarized, e.g. Haiku); on
# Opus 4.7/4.8 the thinking field is almost always empty (display=omitted) so it
# almost never matches. Signal 8.
THRASH = re.compile(
    r"\b(wait|actually|on second thought|let me reconsider|scratch that|"
    r"i was wrong|that'?s wrong|never ?mind|hold on)\b|hmm,",
    re.I,
)

# Debug logs: ~/.claude/debug/<session>.txt, one per --debug session. Each line
# is "<iso-ts> [LEVEL] <rest>". We extract only the optimize-relevant events
# (auto-mode classifier + permission + dispatch latency, API TTFB, ERROR/WARN),
# never the raw DEBUG firehose, so the frame stays compact. The `latest` symlink
# has no .txt suffix so the *.txt glob skips it (its target is globbed once).
DEBUG_DIR = os.path.expanduser("~/.claude/debug")
DBG_HEAD = re.compile(r"^(\S+)\s+\[(\w+)\]\s+(.*)$")
# classifier_request_started carries the model; _finished carries the duration.
# Join the two by reqId so each finished event is attributed to its model.
DBG_CLASS_START = re.compile(r"\[Stall\] classifier_request_started reqId=(\S+) tool=(\S+) model=(\S+)")
DBG_CLASS_DONE = re.compile(r"\[Stall\] classifier_request_finished reqId=(\S+) tool=(\S+).*?\bdurationMs=(\d+)")
DBG_PERM = re.compile(r"\[Stall\] tool_dispatch_start tool=(\S+).*?\bpermissionDecisionMs=(\d+)")
DBG_DISPATCH = re.compile(r"\[Stall\] tool_dispatch_end tool=(\S+).*?\bdurationMs=(\d+)")
DBG_TTFB = re.compile(r"\[API:timing\] first byte after (\d+)ms")


def parse_ts(s):
    try:
        dt = datetime.fromisoformat((s or "").replace("Z", "+00:00"))
        # Force timezone-aware (UTC) so naive + aware timestamps never mix in a
        # subtraction (a single naive transcript ts would otherwise TypeError).
        return dt if dt.tzinfo is not None else dt.replace(tzinfo=timezone.utc)
    except Exception:
        return None


def _row_schema(pl):
    # Explicit schema so an empty scan window still yields a frame WITH columns
    # (a schema-less pl.DataFrame([]) has no "kind" column and later filters crash).
    return {
        "session": pl.String, "ts": pl.Datetime("us", "UTC"), "kind": pl.String,
        "model": pl.String, "speed": pl.String, "n_iter": pl.Int64, "out_tok": pl.Int64,
        "n_tooluse": pl.Int64, "n_think": pl.Int64, "n_think_text": pl.Int64,
        "think_chars": pl.Int64, "is_sidechain": pl.Boolean,
    }


def _debug_schema(pl):
    # One row per timing/error event extracted from a --debug session log.
    # kind in: classifier, permission, dispatch, ttfb, log. ms is the latency in
    # milliseconds (null for log rows); level/msg are set only for log rows.
    return {
        "session": pl.String, "ts": pl.Datetime("us", "UTC"), "kind": pl.String,
        "tool": pl.String, "model": pl.String, "ms": pl.Int64,
        "level": pl.String, "msg": pl.String,
    }


def is_real_prompt(r, content) -> bool:
    if not isinstance(content, str) or not r.get("promptId"):
        return False
    return not content.lstrip().lower().startswith(HARNESS_PREFIXES)


def strip_prefix(cmd: str) -> str:
    c = cmd.strip()
    c = re.sub(r"^(cd\s+\S+\s*(&&|;)\s*)+", "", c)
    c = re.sub(r"^([A-Z_][A-Z0-9_]*=\S+\s+)+", "", c)
    return c.strip()


def klass(cmd: str) -> str:
    c = strip_prefix(cmd)
    for k in ("nix build", "nix run", "nix flake", "nix develop", "home-manager",
              "darwin-rebuild", "cargo", "just", "git", "gh", "claude", "rg", "grep",
              "fd", "npm", "pnpm", "bun", "python", "uv", "ssh", "docker", "kubectl"):
        if c == k or c.startswith(k + " "):
            return k
    return (c.split() or ["?"])[0][:24]


def norm_cmd(cmd: str) -> str:
    """Canonicalize a command so the same task across sessions collapses to one key."""
    c = strip_prefix(cmd)
    c = re.sub(r"/[\w./-]+", "<path>", c)
    c = re.sub(r"\b[0-9a-f]{7,40}\b", "<hash>", c)
    c = re.sub(r"\b\d+\b", "#", c)
    c = re.sub(r"\s+", " ", c).strip()
    return c[:90]


def _cstr(c) -> str:
    return c if isinstance(c, str) else json.dumps(c, default=str)


def _scan_debug(days: int, full: bool):
    """Extract optimize-relevant timing/error events from --debug session logs.

    Returns (rows, n_files). Sparse by nature: empty when no --debug session
    exists in the window. Same mtime window as the transcript scan.
    """
    files = glob.glob(os.path.join(DEBUG_DIR, "*.txt"))
    if not full:
        cutoff = datetime.now(timezone.utc).timestamp() - days * 86400
        files = [f for f in files if os.path.getmtime(f) >= cutoff]
    rows = []
    for f in files:
        sess = os.path.basename(f)[:-4]  # strip ".txt" -> session id
        try:
            lines = open(f, encoding="utf-8").read().splitlines()
        except Exception:
            continue
        models = {}  # reqId -> model, captured from classifier_request_started
        for l in lines:
            h = DBG_HEAD.match(l)
            if not h:
                continue
            # One malformed line must not abort the file.
            try:
                ts, level, rest = parse_ts(h.group(1)), h.group(2), h.group(3)
                m = DBG_CLASS_START.search(rest)
                if m:
                    models[m.group(1)] = m.group(3)
                    continue
                m = DBG_CLASS_DONE.search(rest)
                if m:
                    rows.append(dict(session=sess, ts=ts, kind="classifier",
                                     tool=m.group(2), model=models.get(m.group(1)),
                                     ms=int(m.group(3)), level=None, msg=None))
                    continue
                m = DBG_PERM.search(rest)
                if m:
                    rows.append(dict(session=sess, ts=ts, kind="permission",
                                     tool=m.group(1), model=None, ms=int(m.group(2)),
                                     level=None, msg=None))
                    continue
                m = DBG_DISPATCH.search(rest)
                if m:
                    rows.append(dict(session=sess, ts=ts, kind="dispatch",
                                     tool=m.group(1), model=None, ms=int(m.group(2)),
                                     level=None, msg=None))
                    continue
                m = DBG_TTFB.search(rest)
                if m:
                    rows.append(dict(session=sess, ts=ts, kind="ttfb", tool=None,
                                     model=None, ms=int(m.group(1)), level=None, msg=None))
                    continue
                if level in ("ERROR", "WARN"):
                    rows.append(dict(session=sess, ts=ts, kind="log", tool=None,
                                     model=None, ms=None, level=level, msg=rest[:200]))
            except Exception:
                continue
    return rows, len(files)


def build_frames(days: int = 45, full: bool = False):
    import polars as pl
    files = glob.glob(os.path.join(PROJECTS, "*", "*.jsonl"))
    if not full:
        cutoff = datetime.now(timezone.utc).timestamp() - days * 86400
        files = [f for f in files if os.path.getmtime(f) >= cutoff]

    rows, tool_calls, bash_done, tool_results, corrections, chains = [], {}, [], [], [], []
    # Thinking-thrash tally keyed on (session, model) -> [text_turns,
    # thrash_turns, thrash_hits]. Keyed by model too so a mixed-model session is
    # attributed exactly (not lumped onto whichever model thought last).
    # Accumulated during the scan so the aggregate can be built WITHOUT ever
    # holding raw thinking text in a frame (signal 8).
    thrash = {}
    for f in files:
        sess = os.path.basename(f)[:-6]
        try:
            lines = open(f, encoding="utf-8").read().splitlines()
        except Exception:
            continue
        run_tools = run_errs = 0
        for l in lines:
            if not l.strip():
                continue
            # One malformed/unexpected record must not abort the whole scan.
            try:
                r = json.loads(l)
                t = r.get("type")
                m = r.get("message") if isinstance(r.get("message"), dict) else {}
                ts = parse_ts(r.get("timestamp", ""))
                content = m.get("content")
                if t == "assistant":
                    blocks = content if isinstance(content, list) else []
                    tu = [b for b in blocks if isinstance(b, dict) and b.get("type") == "tool_use"]
                    # n_think counts ALL thinking blocks (incl. Opus 4.7/4.8's
                    # signature-only, display=omitted blocks whose `thinking` is
                    # ""); n_think_text + think_chars measure only VISIBLE
                    # reasoning, so the omitted-vs-summarized split is legible.
                    # Coerce non-str `thinking` to "" so one weird record loses
                    # only its thinking chars, not the whole row to the except.
                    think = [b.get("thinking") if isinstance(b.get("thinking"), str) else ""
                             for b in blocks
                             if isinstance(b, dict) and b.get("type") == "thinking"]
                    th = len(think)
                    th_text = sum(1 for s in think if s)
                    th_chars = sum(len(s) for s in think)
                    u = m.get("usage") or {}
                    it = u.get("iterations")
                    run_tools += len(tu)
                    model = m.get("model")
                    rows.append(dict(
                        session=sess, ts=ts, kind="assistant", model=model,
                        speed=u.get("speed"), n_iter=(len(it) if isinstance(it, list) else 0),
                        out_tok=u.get("output_tokens"), n_tooluse=len(tu), n_think=th,
                        n_think_text=th_text, think_chars=th_chars,
                        is_sidechain=bool(r.get("isSidechain")),
                    ))
                    # Tally thrash over visible thinking only; raw text is dropped here.
                    if th_text:
                        joined = "\n".join(s for s in think if s)
                        hits = len(THRASH.findall(joined))
                        agg = thrash.get((sess, model)) or [0, 0, 0]
                        agg[0] += 1                         # turns with visible thinking
                        agg[1] += 1 if hits else 0          # turns that backtrack
                        agg[2] += hits                      # total marker hits
                        thrash[(sess, model)] = agg
                    for b in tu:
                        inp = b.get("input") or {}
                        label = (inp.get("command") or inp.get("file_path") or inp.get("pattern")
                                 or inp.get("query") or inp.get("description") or "")
                        tool_calls[b.get("id")] = (ts, b.get("name"), str(label)[:200], sess)
                elif t == "user":
                    blocks = content if isinstance(content, list) else None
                    if blocks:
                        for b in blocks:
                            if not (isinstance(b, dict) and b.get("type") == "tool_result"):
                                continue
                            tid = b.get("tool_use_id")
                            size = len(_cstr(b.get("content")))
                            err = bool(b.get("is_error"))
                            ts0, name, label, _ = tool_calls.get(tid, (None, "?", "", sess))
                            tool_results.append((sess, name, label, size, err))
                            if err:
                                run_errs += 1
                            if name == "Bash" and ts is not None and ts0 is not None:
                                bash_done.append((sess, label, (ts - ts0).total_seconds(), err))
                    elif is_real_prompt(r, content):
                        if run_tools >= 8 and run_errs >= 1:
                            chains.append(dict(session=sess, tools=run_tools, errors=run_errs,
                                               next_prompt=content[:160]))
                        if CORRECTION.search(content) and run_tools >= 1:
                            corrections.append(dict(session=sess, ts=str(ts), prior_tools=run_tools,
                                                    prior_errors=run_errs, prompt=content[:200]))
                        run_tools = run_errs = 0
            except Exception:
                continue

    debug_rows, debug_files = _scan_debug(days, full)

    return dict(
        df=pl.DataFrame(rows, schema=_row_schema(pl)),
        bash=pl.DataFrame(bash_done, schema=["session", "cmd", "seconds", "is_error"], orient="row"),
        tools=pl.DataFrame(tool_results, schema=["session", "tool", "label", "size", "is_error"], orient="row"),
        debug=pl.DataFrame(debug_rows, schema=_debug_schema(pl)),
        corrections=corrections, chains=chains,
        thrash=[dict(session=s, model=mdl, think_turns=v[0],
                     thrash_turns=v[1], thrash_hits=v[2])
                for (s, mdl), v in thrash.items()],
        files=len(files), debug_files=debug_files,
        window=("full" if full else f"{days}d"),
    )


def thinking_traces(session: str | None = None, *, days: int = 45, full: bool = False,
                    model: str | None = None, with_text: bool = True,
                    projects: str = PROJECTS):
    """One row per thinking block as a polars frame: session, ts, model, idx,
    text_len, sig_len, and (when with_text) the summarized reasoning `text` itself.
    `idx` is a per-session monotonic counter in transcript (chronological) order,
    so it gives a stable total order even across blocks that share a `ts`.

    Kept OUT of build_frames on purpose: build_frames only TALLIES thinking
    (n_think / n_think_text / think_chars) so raw reasoning never bloats the
    aggregate frames. Reach here when you actually want to READ the reasoning,
    and SCOPE it — pass `session=` for one transcript (recommended) or a tight
    `days` window — because the text column is exactly the context cost the rest
    of the library avoids. Pass with_text=False for a lengths-only frame.

    Persistence is model- and harness-dependent (see the model loop profile's
    frac_visible_think): Opus 4.7/4.8 store only an encrypted `signature`
    (text_len=0, display=omitted) UNLESS the harness passes
    `--thinking-display summarized` (the index claude-code wrapper does by
    default, PR #576), in which case summarized `text` is present; Haiku 4.5
    always stores text. Either way it is the API's SUMMARY, never raw CoT.
    """
    import polars as pl
    files = glob.glob(os.path.join(projects, "*", "*.jsonl"))
    if session is not None:  # "" is a valid (if nonsensical) id, NOT a fall-through to the window scan
        files = [f for f in files if os.path.basename(f)[:-6] == session]
    elif not full:
        cutoff = datetime.now(timezone.utc).timestamp() - days * 86400
        files = [f for f in files if os.path.getmtime(f) >= cutoff]
    rows = []
    for f in files:
        sess = os.path.basename(f)[:-6]
        try:
            lines = open(f, encoding="utf-8").read().splitlines()
        except Exception:
            continue
        # Per-session monotonic block counter in transcript order. Claude Code
        # writes one thinking block per record, so a per-RECORD index would be
        # always-0 and useless; counting across the session makes idx a real,
        # collision-free ordinal that disambiguates same-timestamp blocks.
        idx = 0
        for l in lines:
            # Cheap prefilter: a thinking block's line always contains the
            # substring; skip the json.loads on the ~majority of lines without one.
            if '"thinking"' not in l:
                continue
            try:
                r = json.loads(l)
                if r.get("type") != "assistant":
                    continue
                m = r.get("message") if isinstance(r.get("message"), dict) else {}
                if not isinstance(m.get("content"), list):
                    continue
                mdl = m.get("model")
                if model and mdl != model:
                    continue
                ts = parse_ts(r.get("timestamp", ""))
                for b in m["content"]:
                    if not (isinstance(b, dict) and b.get("type") == "thinking"):
                        continue
                    txt = b.get("thinking") if isinstance(b.get("thinking"), str) else ""
                    sig = b.get("signature") if isinstance(b.get("signature"), str) else ""
                    row = dict(session=sess, ts=ts, model=mdl, idx=idx,
                               text_len=len(txt), sig_len=len(sig))
                    if with_text:
                        row["text"] = txt
                    rows.append(row)
                    idx += 1
            except Exception:
                continue
    schema = {"session": pl.String, "ts": pl.Datetime("us", "UTC"), "model": pl.String,
              "idx": pl.Int64, "text_len": pl.Int64, "sig_len": pl.Int64}
    if with_text:
        schema["text"] = pl.String
    # (session, idx) is unique + monotonic, so the sort is deterministic.
    return pl.DataFrame(rows, schema=schema).sort(["session", "idx"])


def summary_line(F):
    df = F["df"]
    dbg = F.get("debug")
    return (f"window: {F['window']} | files={F['files']} | rows={df.height} "
            f"| timed_bash={F['bash'].height} | tool_results={F['tools'].height} "
            f"| debug_files={F.get('debug_files', 0)} "
            f"debug_events={dbg.height if dbg is not None else 0}")


def aggregates(F, top: int = 15, oversize: int = 20000):
    """Return an ordered list of (title, polars-frame) sections shared by the text
    report and the HTML report, so the two never drift."""
    import polars as pl
    df, bash, tools = F["df"], F["bash"], F["tools"]
    out = []

    a = df.filter((pl.col("kind") == "assistant") & pl.col("model").is_not_null()
                  & (pl.col("model") != "<synthetic>"))
    if a.height:
        # avg_think counts ALL thinking blocks (Opus 4.7/4.8 emit empty,
        # signature-only ones with display=omitted, so it looks high while no
        # reasoning is visible). avg_think_chars / frac_visible_think measure
        # only VISIBLE text, exposing the omitted(~0) vs summarized(>0) split.
        out.append(("model loop profile", a.group_by("model").agg(
            pl.len().alias("turns"), pl.col("out_tok").median().alias("med_out_tok"),
            pl.col("n_tooluse").mean().round(2).alias("avg_tooluse"),
            pl.col("n_think").mean().round(2).alias("avg_think"),
            pl.col("think_chars").mean().round(1).alias("avg_think_chars"),
            (pl.col("n_think_text") > 0).mean().round(3).alias("frac_visible_think"),
            (pl.col("speed") == "fast").mean().round(3).alias("frac_fast"),
        ).sort("turns", descending=True)))

    if bash.height:
        bb = bash.with_columns(pl.col("cmd").map_elements(klass, return_dtype=pl.String).alias("class"))
        out.append(("command classes by cumulative wall-clock (s)", bb.group_by("class").agg(
            pl.len().alias("n"), pl.col("seconds").sum().round(0).alias("total_s"),
            pl.col("seconds").median().round(1).alias("med_s"),
            pl.col("seconds").max().round(0).alias("max_s"),
            pl.col("is_error").sum().alias("errs"),
        ).sort("total_s", descending=True).head(top)))
        out.append(("slowest single commands", bash.sort("seconds", descending=True)
                    .select("seconds", "is_error", "cmd").head(top)))
        out.append(("most repeated error-producing commands", bash.filter("is_error")
                    .with_columns(pl.col("cmd").map_elements(strip_prefix, return_dtype=pl.String).str.slice(0, 60).alias("c"))
                    .group_by("c").agg(pl.len().alias("fails"), pl.col("seconds").sum().round(0).alias("wasted_s"))
                    .sort("fails", descending=True).head(top)))
        out.append(("recurring tasks across sessions (candidates to script)",
                    bash.with_columns(pl.col("cmd").map_elements(norm_cmd, return_dtype=pl.String).alias("task"))
                    .group_by("task").agg(pl.col("session").n_unique().alias("sessions"), pl.len().alias("runs"),
                                          pl.col("seconds").sum().round(0).alias("total_s"))
                    .filter(pl.col("sessions") >= 3).sort(["sessions", "total_s"], descending=True).head(top)))
        # signal 7: escape hatches / jank -> arch-fix candidates
        jrows = []
        for label, pat in JANK:
            mask = bash.filter(pl.col("cmd").map_elements(lambda c, p=pat: bool(p.search(c)), return_dtype=pl.Boolean))
            if mask.height:
                jrows.append((label, mask.height, mask["session"].n_unique(), round(mask["seconds"].sum())))
        if jrows:
            out.append(("escape hatches / jank (arch-fix candidates)",
                        pl.DataFrame(jrows, schema=["jank", "runs", "sessions", "total_s"], orient="row")
                        .sort(["sessions", "runs"], descending=True)))

    if tools.height:
        out.append(("context bloat by tool", tools.group_by("tool").agg(
            pl.len().alias("calls"), (pl.col("size") >= oversize).sum().alias("oversized"),
            pl.col("size").max().alias("max_bytes"), pl.col("size").sum().alias("total_bytes"),
        ).sort("total_bytes", descending=True).head(top)))
        out.append(("biggest single results (scope these)", tools.sort("size", descending=True)
                    .select("tool", "size", "label").head(top)))

    if F["corrections"]:
        out.append((f"human corrections ({len(F['corrections'])} total)",
                    pl.DataFrame(F["corrections"]).sort("prior_tools", descending=True)
                    .select("session", "prior_tools", "prior_errors", "prompt").head(top)))
    if F["chains"]:
        out.append((f"long autonomous chains w/ errors ({len(F['chains'])} total)",
                    pl.DataFrame(F["chains"]).sort(["errors", "tools"], descending=True).head(top)))

    # signal 8: thinking thrash / backtracking. Only sessions with VISIBLE
    # thinking text contribute (display=summarized; omitted-thinking models like
    # Opus 4.7/4.8 emit no text so never appear). frac = thrash_turns/think_turns.
    if F.get("thrash"):
        tw = pl.DataFrame(F["thrash"]).filter(pl.col("think_turns") > 0)
        if tw.height:
            out.append(("thinking thrash by model (visible-reasoning turns)",
                        tw.group_by("model").agg(
                            pl.col("think_turns").sum().alias("think_turns"),
                            pl.col("thrash_turns").sum().alias("thrash_turns"),
                            pl.col("thrash_hits").sum().alias("thrash_hits"),
                        ).with_columns((pl.col("thrash_turns") / pl.col("think_turns"))
                                       .round(3).alias("frac_thrash"))
                        .sort("thrash_turns", descending=True)))
            # Per-session view is about which sessions thrash; sum across any
            # models that thought in the session (no model column, see M2).
            out.append(("thinking thrash by session (most backtracking)",
                        tw.group_by("session").agg(
                            pl.col("think_turns").sum().alias("think_turns"),
                            pl.col("thrash_turns").sum().alias("thrash_turns"),
                            pl.col("thrash_hits").sum().alias("thrash_hits"),
                        ).with_columns((pl.col("thrash_turns") / pl.col("think_turns"))
                                       .round(3).alias("frac_thrash"))
                        .sort(["thrash_turns", "thrash_hits"], descending=True).head(top)))

    # signal 9: harness / auto-mode overhead, from --debug session logs only.
    # The transcript wall-clock hides this: every tool call pays a classifier
    # request (auto-mode) + a permission decision before it runs. classifier_s
    # is per-tool cumulative auto-mode latency; a tool with many calls and a
    # high classifier_s is paying a fixed harness tax worth reducing (fewer
    # calls, batch, or reconsider auto-mode for that tool). Sparse: present only
    # if the user ran --debug sessions in the window.
    dbg = F.get("debug")
    if dbg is not None and dbg.height:
        ov = dbg.filter(pl.col("kind").is_in(["classifier", "permission", "dispatch"]))
        if ov.height:
            # events = total dispatched calls of this tool (never 0, so a tool
            # that ran but was not auto-mode-classified reads as classifier_calls=0,
            # not "never ran"). overhead_s = classifier + permission latency only
            # (the harness tax); dispatch_s is the tool's own runtime, shown for
            # context. sum-of-empty is 0.0 (never null), so sort needs no nulls_last.
            out.append(("auto-mode overhead by tool (harness latency, debug logs)",
                        ov.group_by("tool").agg(
                            pl.len().alias("events"),
                            pl.col("ms").filter(pl.col("kind") == "classifier").len().alias("classifier_calls"),
                            (pl.col("ms").filter(pl.col("kind") == "classifier").sum() / 1000).round(1).alias("classifier_s"),
                            pl.col("ms").filter(pl.col("kind") == "classifier").median().round(0).alias("med_class_ms"),
                            (pl.col("ms").filter(pl.col("kind") == "permission").sum() / 1000).round(1).alias("perm_s"),
                            (pl.col("ms").filter(pl.col("kind") == "dispatch").sum() / 1000).round(1).alias("dispatch_s"),
                        ).with_columns((pl.col("classifier_s") + pl.col("perm_s")).round(1).alias("overhead_s"))
                        .sort("overhead_s", descending=True).head(top)))
        tf = dbg.filter(pl.col("kind") == "ttfb")
        if tf.height:
            out.append(("API time-to-first-byte (ms, debug logs)", tf.select(
                pl.len().alias("requests"), pl.col("ms").median().round(0).alias("med_ms"),
                pl.col("ms").quantile(0.9).round(0).alias("p90_ms"),
                pl.col("ms").max().alias("max_ms"))))
        lg = dbg.filter(pl.col("kind") == "log")
        if lg.height:
            out.append(("runtime errors / warnings (debug logs)",
                        lg.with_columns(pl.col("msg").str.slice(0, 80).alias("m"))
                        .group_by("level", "m").agg(
                            pl.len().alias("n"), pl.col("session").n_unique().alias("sessions"))
                        .sort("n", descending=True).head(top)))
    return out


def report(F, top: int = 15, oversize: int = 20000):
    import polars as pl
    print(summary_line(F))
    for title, frame in aggregates(F, top, oversize):
        print(f"\n=== {title} ===")
        with pl.Config(tbl_rows=top, tbl_cols=20, fmt_str_lengths=70, tbl_width_chars=160):
            print(frame)


def write_html(F, path, top: int = 15, oversize: int = 20000):
    import polars as pl
    path = os.path.expanduser(path)
    css = ("body{font:13px ui-monospace,SFMono-Regular,Menlo,monospace;margin:2rem;"
           "background:#fff;color:#111}h1{font-size:18px}h2{font-size:14px;margin-top:2rem;"
           "border-bottom:1px solid #ddd;padding-bottom:.3rem}table{border-collapse:collapse;"
           "font-size:12px}th,td{border:1px solid #ddd;padding:3px 8px;text-align:left}"
           "th{background:#f4f4f4}@media(prefers-color-scheme:dark){body{background:#0d0d0d;"
           "color:#ddd}h2{border-color:#333}th,td{border-color:#333}th{background:#1a1a1a}}")
    parts = [f"<!doctype html><html><head><meta charset=utf-8><title>optimize report</title>"
             f"<style>{css}</style></head><body>",
             f"<h1>optimize report</h1><p>{html.escape(summary_line(F))}</p>"]
    with pl.Config(tbl_rows=top, fmt_str_lengths=120):
        for title, frame in aggregates(F, top, oversize):
            parts.append(f"<h2>{html.escape(title)}</h2>")
            parts.append(frame._repr_html_())
    parts.append("</body></html>")
    with open(path, "w") as fh:
        fh.write("".join(parts))
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--days", type=int, default=45)
    ap.add_argument("--full", action="store_true")
    ap.add_argument("--out", default=os.path.expanduser("~/.claude/optimize"))
    ap.add_argument("--top", type=int, default=15)
    ap.add_argument("--oversize", type=int, default=20000)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    F = build_frames(days=args.days, full=args.full)
    report(F, top=args.top, oversize=args.oversize)
    F["df"].write_parquet(os.path.join(args.out, "history_rows.parquet"))
    F["bash"].write_parquet(os.path.join(args.out, "history_bash.parquet"))
    F["tools"].write_parquet(os.path.join(args.out, "history_tools.parquet"))
    F["debug"].write_parquet(os.path.join(args.out, "history_debug.parquet"))
    with open(os.path.join(args.out, "findings.json"), "w") as fh:
        json.dump({k: F[k] for k in ("window", "files", "debug_files", "corrections", "chains", "thrash")}, fh, indent=2)
    write_html(F, os.path.join(args.out, "report.html"), top=args.top, oversize=args.oversize)
    print(f"\nwrote parquet caches + findings.json + report.html to {args.out}")


if __name__ == "__main__":
    sys.exit(main())
