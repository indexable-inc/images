---
description: Draft tweets using research-backed virality patterns
argument-hint: <topic, draft, or link>
---

# Tweet

Draft 3 tweet variants for `$ARGUMENTS` using the patterns below. Each variant must pick a different format. After drafting, name which principle each one leverages so the user can pick.

Default to text-only. No hashtags unless `$ARGUMENTS` is breaking-news bait. No links inside the main tweet — drop them in a reply if needed. No all-caps. No corporate lead-ins ("We're thrilled…"), no hedging ("I just wanted to share…"), no throat-clearing ("So basically…").

## What the research actually says

Use these as load-bearing rules, not vibes. Cite the source in the rationale if the user asks why a variant is shaped a certain way.

> Berger & Milkman, *Journal of Marketing Research* (2012), "What Makes Online Content Viral?" — analysis of ~7,000 NYT articles' "most emailed" lists. High-arousal emotion (awe, anger, anxiety, amusement) drives sharing; low-arousal (sadness, contentment) suppresses it. **Anger was the single strongest predictor.** https://jonahberger.com/wp-content/uploads/2013/02/ViralityB.pdf

→ Inference: pick a high-arousal emotion before writing. Awe and amusement are the safer positive bets; anger/outrage works but burns goodwill.

> Warwick Business School working paper, "Concrete Language Enhances Sharing of Social Media Posts" — 15M tweets + Reddit corpus + pre-registered experiment. **More concrete posts get more retweets and upvotes**, with consistent effects also for words acquired later in life and rated more arousing. https://wrap.warwick.ac.uk/192325/2/WRAP-Concrete-language-sharing-social-media-posts-Twitter-Reddit-25.pdf

→ Inference: prefer concrete nouns and specific numbers over abstractions. "$4,237 last month" beats "a lot of money." Name the thing instead of gesturing.

> Tan, Lee, Pang, arXiv:1405.1438 — paired-tweet natural experiment controlling for topic and author. **Wording alone significantly changes propagation.** Predictors include conforming to community language while standing out lexically, and matching the author's prior style. https://ar5iv.labs.arxiv.org/html/1405.1438

→ Inference: don't trust the topic to carry the tweet; wording is a real lever even at fixed content.

> Zhang et al., arXiv:1706.07484 — 122M engagements on 2.5M tweets. Tweet-feature relationships with effectiveness are **non-linear**: a couple of hashtags beats zero or many; very short and very long tweets both underperform mid-length. https://arxiv.org/pdf/1706.07484

→ Inference: zero or one hashtag, not three. Aim for one full thought, not a fragment and not a wall.

> Guo et al., NAACL-LSM, "What Makes a Tweet Worth Sharing" — surface, parse-based, and Twitter-specific features. **Opinion tweets favor originality and pithiness**; update tweets favor direct statements of current activity. https://www.comp.nus.edu.sg/~kanmy/papers/NAACL-LSM.pdf

→ Inference: if it's an opinion, sharpen and shorten. If it's a status/update, be direct and concrete; don't dress it up.

> X published ranking weights (2026), as documented by Social Media Today and the open-sourced xAI recommendation stack — **a reply that the author replies back to scores +75 vs +0.5 for a like (150x).** Bookmarks and quote posts outweigh likes. External links, all-caps, and zero first-hour engagement suppress distribution. https://postory.io/blog/what-goes-viral-on-twitter

→ Inference: write something the author can plausibly reply to. Take a side so people argue or co-sign. Keep links out of the primary post.

> Practitioner synthesis (WildandFree, Postory, Tweet Archivist — 2025–2026) cross-checked against the above papers — the six emotions that reliably trigger shares are **validation, surprise, humor, outrage, aspiration, usefulness**. The first 5–7 words decide whether the tweet survives. https://wildandfreetools.com/blog/what-makes-a-tweet-go-viral/ and https://postory.io/blog/viral-tweets

→ Inference: the hook does most of the work. If line one doesn't land, the rest is invisible.

## Calibrate to author voice FIRST

Tan/Lee/Pang found "matching the author's prior style" is a real propagation lever. If you skip this step, you will default to a generic LinkedIn-thought-leader voice and the tweet will read as not-by-them.

Before drafting, sample the author's recent tweets (ask for a paste, or pull from a saved sample) and note:

- **Capitalization**: do they use lowercase by default? Title Case? Sentence case?
- **Length distribution**: are most of their tweets one fragment, one sentence, or multi-line?
- **Default register**: observation (status), opinion (take), or builder log (TIL/shipping)? Match the register of the source material, not the most viral format.
- **Endings**: how do they close? Trailing thought ("hmmmm"), personal action ("doing X now"), nothing, or a CTA? CTAs and morals ("stop doing X", "you need Y") almost never appear in casual technical voices.
- **Punctuation habits**: em dashes, ellipses, periods or no periods at end of fragments.
- **Slang/community markers**: lowercase brand names, project shorthand, in-group references.

If no samples are available, ask for 3–5 recent tweets before drafting. Do not guess voice from the username or bio.

## The five formats (pick a different one per variant)

Pick formats that fit the **source material's register**, not the most viral-looking template. If the user handed you a neutral observation, do not force a contrarian take or a moralizing one-liner.

1. **One-liner observation** — single sentence stating what you noticed. Best for status/builder voices.
2. **One-liner truth bomb** — single sharp sentence that's quote-tweetable. Best for opinion + validation.
3. **Story** — first-person micro-narrative with a payoff in the last line. Best for concrete + surprise.
4. **Numbered list** — 3–7 items, one per line, each standalone-useful. Best for usefulness + bookmarks.
5. **Contrarian take** — confident claim against a common belief, no hedging. Best for reply-chains + outrage/validation. Only use if the source material is actually an opinion.
6. **TIL / builder log** — "TIL X" or "shipped Y" framing. Best for status updates from people who post in this register.
7. **Screenshot tweet** — text framing a chart/DM/receipt/quote. Best when the claim needs proof. (Note in the rationale what image to attach.)

## The hook patterns (use one per variant)

- Curiosity gap: "Most people are doing X wrong and don't know it…"
- Specific bold claim: "I went from 200 to 15,000 followers in 4 months. Here's what worked."
- Unexpected contrast: "I fired my best employee. It was the best decision I ever made."

## Workflow

1. **Voice check**: if you don't have author samples in this conversation, ask for 3–5 recent tweets (or a profile paste) before doing anything else. Without samples, drafts default to generic.
2. If `$ARGUMENTS` is empty or just a topic, ask one round of clarifying questions via AskUserQuestion: audience, intended emotion (validation / surprise / humor / outrage / aspiration / usefulness), and any concrete numbers or named examples available. Skip this step if the user supplied a draft.
3. Generate 3 variants, each using a different format from the list above. At least one variant should hew closest to the author's most common format, even if it's the least "viral-looking" option.
4. For each variant, output:
   - The tweet text (under 280 chars, count it).
   - One line: format + hook pattern + emotion + which research principle it's leaning on + which author-voice trait it matches.
5. Name your pick and why, in one sentence.
6. End with one sentence on what to do in the first hour (reply to every reply, that's the +75 signal).

## Self-check before output

Reject any variant that:
- Starts with hedging, throat-clearing, or a corporate lead-in.
- Uses vague nouns where a concrete one is available ("things," "stuff," "people").
- Uses a rounded or vague number where a specific one would work.
- Contains a link in the main tweet.
- Is balanced or neutral when an opinion was asked for; balanced takes die.
- Could be scrolled past without any reaction.
- **Lectures or moralizes when the source was just an observation.** "Stop doing X", "you need to Y", "the question is wrong" patterns turn observations into takes the author didn't make. If the source is "I noticed X", the tweet is "I noticed X", not "and here's what you should learn from it."
- **Doesn't match author voice.** If they write lowercase and you wrote Title Case, redo it. If they end with trailing thoughts and you ended with a CTA, redo it. If they don't normally moralize and you added a moral, cut it.
- Contains em dashes. Rewrite the sentence.
