defmodule IxMcp.RLM do
  @moduledoc ~S"""
  Recursive-language-model primitives for the kernel: what they are, why
  they are shaped this way, and where they improve on the paper.

  ## The idea

  From *Recursive Language Models* (arXiv:2512.24601). A model with a fixed
  window degrades as context grows, and the usual answers -- summarize
  first, retrieve top-k, buy a bigger window -- all decide in advance what
  matters. RLM refuses the framing: the context is never ingested. It sits
  in a REPL as a VARIABLE, the model writes code to peek at it, and it makes
  recursive calls to itself over slices it chose. Cost tracks the parts
  examined, not the size of the input.

  The kernel was already most of the way there: `IxMcp.Workspace` is a
  persistent Elixir REPL where bindings survive across cells, which is
  exactly the substrate the paper describes. Three primitives were missing.

    * `IxMcp.Ctx` — a handle on content that RENDERS AS METADATA. This is the
      load-bearing trick. A cell that returns a 40 MB log must cost the
      caller one line, or the whole scheme collapses back into ingesting the
      context. Bytes cross into a window only through `Ctx.read/2`, capped,
      or by being handed to a sub-model.
    * `IxMcp.LM` — sub-calls over handles, in parallel.
    * `IxMcp.EventLog` — the audit trail, which is also the derivation cache.

  ## Two deliberate improvements

  PARALLELISM. The paper's implementation blocks on each sub-call in turn,
  which is an artifact of a single-threaded notebook, not a property of the
  method. Sub-calls over disjoint slices are independent by construction, so
  `IxMcp.LM.map/3` runs them over `Task.async_stream`. On the BEAM this is
  the ordinary way to write it rather than an optimization.

  MEMOIZATION AS DERIVATION. A sub-call is a pure function of (model,
  prompt, context bytes), and all three are content-addressed, so the answer
  can be cached under blake3 of the triple. Two consequences that a
  session-scoped cache does not give:

    * Re-analysing a log that GREW pays only for the chunks that are new,
      because chunking is line-aligned and unchanged chunks keep their ids.
    * Handle ids are `ix_hash::Content` -- the same address jj's `FileId`
      uses over the same bytes (`IxMcp.Blake3` documents what was mirrored
      and pins it with a KAT). So the same bytes attached from a local path
      and, once the jj source lands, from a jj tree produce the same id, and
      a cached answer transfers across sources and machines.

  ## The rules that keep it honest

  RENDER DISCIPLINE. A handle never renders its content. Everything a cell
  gets back is metadata or another handle.

  FAIL CLOSED. An exhausted budget returns `{:error, :budget_exhausted}`.
  Never a truncated context, never a dropped sub-call, never a quietly
  cheaper model. A recursive fan-out is precisely where a silent degradation
  would be both most tempting and least visible.

  DEPTH 1 IN v0. Sub-calls are plain completions with no tools and no nested
  REPL. `mode: :rlm` is reserved and refuses rather than pretending.

  ## Shape of a session

      log   = Ctx.file!("/var/log/huge.log")        #Ctx<9f2ab1c4 41.2 MB ...>
      hits  = Ctx.grep(log, ~r/ERROR/)              # metadata + handles
      parts = Ctx.chunks(log, 32)                   # line-aligned handles

      parts
      |> LM.map(fn _ -> "Any failure here? One line." end, concurrency: 8)
      |> Enum.zip(parts)

      LM.budget()      # what that cost
      EventLog.events(kind: :lm_ask)   # and what it did
  """

  @doc """
  The three primitives, in the order they compose.
  """
  @spec primitives() :: [module()]
  def primitives, do: [IxMcp.Ctx, IxMcp.LM, IxMcp.EventLog]

  @doc "The paper this follows."
  @spec paper() :: String.t()
  def paper, do: "Recursive Language Models, arXiv:2512.24601"
end
