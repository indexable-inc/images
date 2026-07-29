---
name: imessage
description: "Read and send iMessage as the signed-in user from the index kernel (Imsg + Contacts). Use whenever the user wants to text someone, reply to a thread, check what somebody said, search their messages, or have an agent answer on their behalf. Covers the mandatory \"Agent:\" prefix, why plain text beats italics, and the chat.db gotchas that make a naive query return nothing."
---

## iMessage from the kernel

`Imsg` reads `~/Library/Messages/chat.db` read-only and sends through
Messages.app. Both work only on the mac the kernel runs on; elsewhere every
call returns `{:error, _}`. `Contacts` maps a name to the handles `Imsg` takes.

```elixir
Contacts.search("Izzy")            # => phones: ["+16507320923"], emails: [...]
Imsg.recent(with: "+16507320923", limit: 30)
Imsg.send("+16507320923", "Agent: on my way")
```

### Every agent-sent message starts with `Agent: `

No exceptions. The recipient is talking to a person, and a message from that
person's phone that the person did not write is a small deception unless it
says so. The prefix is that disclosure, and it is also what lets the recipient
scroll back later and tell which half of the thread to hold the user to.

Introduce yourself the first time you appear in a thread ("Andrew's agent").
After that the bare prefix is enough.

### Plain text, not italics

Send plain. Unicode math-italic (`𝘈𝘨𝘦𝘯𝘵`) looks like a clever way to set agent
messages apart and is the wrong answer twice over: it is not real italic, which
the user will notice immediately, and screen readers pronounce it as unrelated
mathematical symbols. The `Agent: ` prefix already does the job.

Real formatting does exist -- `Imsg.send(to, text, italic: true)`, also `bold:`,
`underline:`, `strike:` -- and sets the genuine
`__kIMTextItalicAttributeName` attribute that Messages' own Format menu sets.
Reach for it only when the formatting itself is the point, because there is no
API for it: AppleScript's `send` takes a string and drops every attribute, and
the only other route is IMCore, a private framework only a signed helper could
link. So a formatted send drives the UI -- Messages to the front, RTF on the
clipboard, paste, return, clipboard restored. It steals focus for a second or
two and can misfire if the user is typing elsewhere.

### The user gets a banner for every send

`Imsg.send` posts a local notification naming the recipient, because Messages
posts no banner for a message the local machine sent, so an agent texting as
the user is otherwise invisible until they open the thread. Pass
`notify: false` only when sending a burst where each banner would be noise.

### Read the thread before answering

Ask for more than the last message. Threads here run fast and short, so the
last line is usually a fragment of an argument that started ten messages up,
and answering it alone produces a confident reply to the wrong question.
`Imsg.recent(with: handle, limit: 30)` and read it oldest-first.

### chat.db gotchas

- **`text` is often NULL**, the user's own sends in particular, with the real
  body in the `attributedBody` typedstream. Every `Imsg` helper decodes it
  already, but a message therefore cannot be found again by `WHERE text LIKE` --
  use `Imsg.search/2`, which prefilters the typedstream byte-wise.
- **Group chats have opaque hex identifiers**, not names. Select them through a
  participant handle (`with:`), never by chat name.
- **Delivery is asynchronous.** `send` returns when Messages accepts the
  message, not when it arrives. When it matters, read `recent/1` a moment later
  and check the row's `error` flag is 0.
- **Timestamps are Apple epoch** (nanoseconds since 2001-01-01); the helpers
  already convert to localtime.

### Sending on someone's behalf

Match the user's register, not your own: these threads are lowercase, clipped
and unhedged, and a well-punctuated paragraph reads as obviously not them even
with the prefix. Say the substance and stop. Break at natural points into two
or three messages rather than sending one wall of text.

When the user asks you to argue a position, argue it. Ask which they want if it
is unclear, because "reply about whether you agree" and "say only where they
are wrong" produce very different messages, and the second one is usually what
is wanted once a thread is already deep in a disagreement.
