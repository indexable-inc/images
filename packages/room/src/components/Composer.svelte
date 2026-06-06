<script lang="ts">
  // Live chat composer.
  //
  // The composer body is a root `LoroText` per thread (see
  // roomDoc.composerText). Root containers have a stable id across
  // peers, so two windows opening the same thread converge on the
  // same logical text — edits merge through the text CRDT with no
  // snapshot-overwrite, no "last-mirrored" baseline, no
  // whose-write-wins. Local writes go through `composer.update(text)`
  // which diffs against the current value and emits the minimal
  // insert/delete ops.
  //
  // Send is fully optimistic: pressing ⏎ commits every visible
  // surface (textarea, LoroText, typing-presence in Loro, transcript
  // optimistic message) in the same frame. The HTTP POST is pure
  // persistence — its only remaining UI side-effects are the
  // draft→thread router.go for first-of-thread sends and the
  // optimistic-message rollback on failure.
  //
  // Typing presence is one tiny state machine: `setTyping(active)`
  // owns the boolean + the throttled Loro write + the 3-second idle
  // teardown. Callers never touch the timers directly.

  import { onDestroy, untrack } from 'svelte';
  import { get } from 'svelte/store';
  import { avatarColor } from '$lib/avatar';
  import { getCaretCoords, type CaretCoords } from '$lib/caretPos';
  import * as api from '$lib/api';
  import { sendChat, type ChatInputItem } from '$lib/chat';
  import { discardDraft, isDraft, updateDraftText } from '$lib/drafts';
  import { loadIdentity } from '$lib/identity';
  import type { ComposerAttachment, PresenceEntry } from '$lib/loro';
  import { router } from '$lib/router';
  import { roomFor } from '$lib/store';
  import { openThreadPanel } from '$lib/threadPanels';
  import { sidebarActive } from '$lib/ui';
  import IconCheck from '~icons/ph/check';
  import IconFile from '~icons/ph/file-text';
  import IconGitDiff from '~icons/ph/git-diff';
  import IconLightning from '~icons/ph/lightning';
  import IconPlus from '~icons/ph/plus';
  import IconShieldCheck from '~icons/ph/shield-check';
  import IconX from '~icons/ph/x';
  import Avatar from './Avatar.svelte';
  import PickerMenu from './PickerMenu.svelte';
  import PickerMenuItem from './PickerMenuItem.svelte';

  interface Props {
    serverId: string;
    threadId: string;
  }
  interface PeerCaret extends CaretCoords {
    id: string;
    name: string;
    github: string | null;
    color: string;
  }

  let { serverId, threadId }: Props = $props();
  const currentRoom = untrack(() => roomFor(serverId));
  const roomDoc = currentRoom.doc;

  // ThreadDetail keys this component by threadId, so the composer
  // is remounted on every thread switch and a once-at-mount read is
  // the right shape. `untrack` keeps Svelte 5's prop-tracking happy
  // about that — we're deliberately not reactive on threadId here.
  const composer = untrack(() => roomDoc.composerText(threadId));
  const composerImages = untrack(() => roomDoc.composerImages(threadId));
  const self = loadIdentity();

  let text = $state(composer.current());
  let sending = $state(false);
  let textarea = $state<HTMLTextAreaElement>();
  let fileInput = $state<HTMLInputElement>();
  let modelPickerRoot = $state<HTMLDivElement>();
  let approvalPickerRoot = $state<HTMLDivElement>();
  let cursor = $state(0);
  let selectedModel = $state('gpt-5.5');
  let selectedEffort = $state('high');
  let selectedApproval = $state('on-request');
  let modelPickerOpen = $state(false);
  let approvalPickerOpen = $state(false);
  let workspace = $state<api.WorkspaceInfo | null>(null);
  let mentions = $state<Array<{ id: string; name: string; path: string }>>([]);
  let skillInvocations = $state<Array<{ id: string; name: string; path: string }>>([]);
  let skills = $state<api.CodexSkill[]>([]);
  let fileResults = $state<api.FileSearchResult[]>([]);
  let fileSearchError = $state<string | null>(null);

  const APPROVAL_OPTIONS = [
    { value: 'on-request', label: 'Ask' },
    { value: 'on-failure', label: 'On failure' },
    { value: 'untrusted', label: 'Untrusted' },
    { value: 'never', label: 'Never' }
  ];
  const FALLBACK_EFFORTS = ['low', 'medium', 'high', 'xhigh'];
  type PaletteCommand = {
    name: string;
    description: string;
    icon: typeof IconFile;
    run: () => void;
  };
  const SLASH_COMMANDS: PaletteCommand[] = [
    {
      name: 'review',
      description: 'Open code changes',
      icon: IconGitDiff,
      run: () => openThreadPanel('review')
    },
    {
      name: 'files',
      description: 'Browse project files',
      icon: IconFile,
      run: () => openThreadPanel('files')
    },
    {
      name: 'clear',
      description: 'Clear the composer',
      icon: IconX,
      run: () => {
        composer.update('');
        text = '';
        mentions = [];
        skillInvocations = [];
        queueMicrotask(autosize);
      }
    }
  ];

  // Image attachments staged for the next send. The bytes stay in
  // local browser state so pasted screenshots do not enter the
  // server's persisted Loro update log. Mirror the store into local
  // state so the template can reactively render it.
  let attachments = $state(composerImages.current());
  $effect(() => composerImages.list.subscribe((v) => (attachments = v)));

  let attachError = $state<string | null>(null);
  let dragActive = $state(false);

  // Mirrors the server's caps in packages/room-server/src/http.rs.
  // Surfaced inline rather than imported because the limits are
  // policy, not protocol — easier to tune one side without coupling.
  const MAX_ATTACHMENTS = 8;
  const MAX_ATTACHMENT_BYTES = 24 * 1024 * 1024;
  const ACCEPTED_MIME = /^image\/(png|jpe?g|gif|webp|heic|heif|avif|bmp|svg\+xml)$/i;

  // Subscribe to LoroText changes. Both local writes and incoming
  // peer ops fire here. The callback assigns text unconditionally:
  // Svelte 5's $state set is a no-op when the value is identical, so
  // the local-write cycle (bind:value → onInput → composer.update →
  // LoroText commit → subscriber fires with same string → text =
  // same string) terminates on its own. Crucially we must NOT read
  // `text` inside this callback — the subscribe fires the listener
  // synchronously on attach, so a read here would register `text` as
  // a dep of this effect, the effect would re-run on every keystroke,
  // and the re-subscription's initial fire would clobber the just-
  // typed character with the readable's stale value.
  $effect(() =>
    composer.text.subscribe((incoming) => {
      text = incoming;
      queueMicrotask(autosize);
    })
  );

  let presence = $state<PresenceEntry[]>([]);
  $effect(() => roomDoc.presenceList.subscribe((v) => (presence = v)));

  let otherTyping = $derived(
    presence.filter(
      (p) =>
        p.id !== self.id &&
        p.typing_thread_id === threadId &&
        Date.now() - p.last_seen_ms < 8000
    )
  );

  let canSend = $derived(
    (text.trim().length > 0 ||
      attachments.length > 0 ||
      mentions.length > 0 ||
      skillInvocations.length > 0) &&
      !sending
  );
  let selectedApprovalLabel = $derived(
    APPROVAL_OPTIONS.find((option) => option.value === selectedApproval)?.label ?? selectedApproval
  );
  let slashToken = $derived(activeToken('/'));
  let mentionToken = $derived(activeToken('@'));
  let slashMatches = $derived.by<PaletteCommand[]>(() => {
    if (!slashToken) return [];
    const query = slashToken.query.toLowerCase();
    const local = SLASH_COMMANDS.filter((cmd) => cmd.name.startsWith(query));
    const skillCommands = skills
      .filter((skill) => skill.name.toLowerCase().startsWith(query))
      .map((skill) => ({
        name: skill.name,
        description: skill.shortDescription || skill.description || 'Invoke skill',
        icon: IconLightning,
        run: () => addSkill(skill)
      }));
    return [...local, ...skillCommands].slice(0, 10);
  });

  // Per-peer caret coordinates. Derived rather than effect-assigned —
  // the mirror-div lookup is a pure function of (textarea, text,
  // peers). Pass `text` explicitly: textarea.value lags one flush
  // behind our $state, so reading from the DOM would render one
  // keystroke late.
  let peerCarets = $derived.by<PeerCaret[]>(() => {
    if (!textarea) return [];
    const out: PeerCaret[] = [];
    for (const p of otherTyping) {
      if (p.typing_cursor == null) continue;
      const offset = Math.min(p.typing_cursor, text.length);
      try {
        const coords = getCaretCoords(textarea, offset, text);
        out.push({ id: p.id, name: p.name, github: p.github, color: avatarColor(p.name), ...coords });
      } catch {
        // mirror failed (typically before mount) — skip this peer
      }
    }
    return out;
  });

  // Auto-focus on mount. Microtask defers until bind:this fires. Skip
  // during sidebar live-preview: the user is browsing chats with j/k,
  // not committing, and stealing focus would let stray keys land as
  // text. Read sidebarActive once via get() — a reactive read would
  // re-steal focus every time the sidebar toggled.
  $effect(() => {
    if (get(sidebarActive)) return;
    queueMicrotask(() => textarea?.focus());
  });

  $effect(() => {
    api
      .readCodexConfig(serverId)
      .then((res) => {
        selectedEffort = res.config.model_reasoning_effort || selectedEffort;
        if (typeof res.config.approval_policy === 'string') selectedApproval = res.config.approval_policy;
      })
      .catch(() => {});
    api
      .listCodexSkills(serverId)
      .then((next) => (skills = next))
      .catch(() => {});
  });

  $effect(() => {
    const id = threadId;
    if (isDraft(id)) {
      workspace = null;
      return;
    }
    api
      .getThreadWorkspace(serverId, id)
      .then((next) => {
        if (id === threadId) workspace = next;
      })
      .catch(() => {
        if (id === threadId) workspace = null;
      });
  });

  $effect(() => {
    const cwd = workspace?.cwd;
    if (!cwd) return;
    api
      .listCodexSkills(serverId, cwd)
      .then((next) => (skills = next))
      .catch(() => {});
  });

  let fileSearchSeq = 0;
  $effect(() => {
    const token = mentionToken;
    if (!token) {
      fileResults = [];
      fileSearchError = null;
      return;
    }
    const seq = ++fileSearchSeq;
    const query = token.query.trim();
    api
      .searchFiles(serverId, query, workspace?.cwd ?? null)
      .then((results) => {
        if (seq === fileSearchSeq) fileResults = results.slice(0, 8);
      })
      .catch((err) => {
        if (seq === fileSearchSeq) fileSearchError = (err as Error).message;
      });
  });

  function autosize() {
    if (!textarea) return;
    textarea.style.height = 'auto';
    const cap = Math.max(240, Math.floor(window.innerHeight * 0.5));
    textarea.style.height = Math.min(Math.max(textarea.scrollHeight, 24), cap) + 'px';
  }

  function activeToken(prefix: '/' | '@'): { start: number; end: number; query: string } | null {
    const pos = Math.max(0, Math.min(cursor, text.length));
    const before = text.slice(0, pos);
    const start = Math.max(before.lastIndexOf(' '), before.lastIndexOf('\n')) + 1;
    const token = before.slice(start);
    if (!token.startsWith(prefix)) return null;
    if (token.length > 1 && /\s/.test(token)) return null;
    return { start, end: pos, query: token.slice(1) };
  }

  function replaceActiveToken(token: { start: number; end: number }, replacement = '') {
    text = text.slice(0, token.start) + replacement + text.slice(token.end);
    composer.update(text);
    cursor = token.start + replacement.length;
    queueMicrotask(() => {
      if (!textarea) return;
      textarea.focus();
      textarea.setSelectionRange(cursor, cursor);
      autosize();
    });
  }

  function runSlashCommand(command: PaletteCommand) {
    const token = slashToken;
    if (token) replaceActiveToken(token, '');
    command.run();
  }

  function addMention(result: api.FileSearchResult) {
    const token = mentionToken;
    mentions = [
      ...mentions.filter((m) => m.path !== result.path),
      { id: crypto.randomUUID(), name: result.file_name || result.path, path: result.path }
    ];
    if (token) replaceActiveToken(token, '');
  }

  function removeMention(id: string) {
    mentions = mentions.filter((m) => m.id !== id);
  }

  function addSkill(skill: api.CodexSkill) {
    const token = slashToken;
    skillInvocations = [
      ...skillInvocations.filter((s) => s.name !== skill.name),
      { id: crypto.randomUUID(), name: skill.name, path: skill.path }
    ];
    if (token) replaceActiveToken(token, '');
  }

  function removeSkill(id: string) {
    skillInvocations = skillInvocations.filter((s) => s.id !== id);
  }

  function effortLabel(effort: string): string {
    if (effort === 'xhigh') return 'Extra High';
    return effort[0].toUpperCase() + effort.slice(1);
  }

  function selectEffort(effort: string) {
    selectedEffort = effort;
    closePickers();
  }

  function toggleModelPicker() {
    modelPickerOpen = !modelPickerOpen;
    approvalPickerOpen = false;
  }

  function toggleApprovalPicker() {
    approvalPickerOpen = !approvalPickerOpen;
    modelPickerOpen = false;
  }

  function selectApproval(value: string) {
    selectedApproval = value;
    closePickers();
  }

  function closePickers() {
    modelPickerOpen = false;
    approvalPickerOpen = false;
    queueMicrotask(() => textarea?.focus());
  }

  function onWindowPointerDown(e: PointerEvent) {
    if (!modelPickerOpen && !approvalPickerOpen) return;
    const target = e.target;
    if (!(target instanceof Node)) return;
    if (modelPickerRoot?.contains(target) || approvalPickerRoot?.contains(target)) return;
    closePickers();
  }

  // -- Typing presence ------------------------------------------------
  //
  // One entry point: setTyping(active). It owns:
  //   - the `typing` boolean,
  //   - throttled Loro publishes (≤10 Hz via the flushTimer), and
  //   - the 3-second idle teardown.
  //
  // The text itself isn't broadcast here — peers read it directly
  // from the shared LoroText. Only the *position* of our caret
  // ships through presence so peers can draw our cursor.

  const TYPING_BROADCAST_MS = 100;
  const TYPING_IDLE_MS = 3000;

  let typing = $state(false);
  let lastPublishedAt = 0;
  let flushTimer: ReturnType<typeof setTimeout> | null = null;
  let idleTimer: ReturnType<typeof setTimeout> | null = null;

  function publishTyping() {
    flushTimer = null;
    lastPublishedAt = Date.now();
    roomDoc.setSelf(self, {
      online: true,
      typing_thread_id: typing ? threadId : null,
      typing_cursor: typing && textarea ? textarea.selectionStart : null
    });
  }

  function scheduleFlush() {
    if (flushTimer !== null) return;
    const wait = Math.max(0, TYPING_BROADCAST_MS - (Date.now() - lastPublishedAt));
    if (wait === 0) publishTyping();
    else flushTimer = setTimeout(publishTyping, wait);
  }

  function setTyping(active: boolean) {
    if (active) {
      typing = true;
      scheduleFlush();
      if (idleTimer) clearTimeout(idleTimer);
      idleTimer = setTimeout(() => setTyping(false), TYPING_IDLE_MS);
    } else {
      typing = false;
      if (flushTimer) {
        clearTimeout(flushTimer);
        flushTimer = null;
      }
      if (idleTimer) {
        clearTimeout(idleTimer);
        idleTimer = null;
      }
      publishTyping();
    }
  }

  onDestroy(() => {
    if (flushTimer) clearTimeout(flushTimer);
    if (idleTimer) clearTimeout(idleTimer);
  });

  // -- Attachments ----------------------------------------------------
  //
  // Three entry points: the paperclip button (file picker), pasting an
  // image into the textarea (clipboard), and drag/drop onto the box.
  // All converge on `addFiles` which validates type + size, encodes to
  // a data URL, and appends to `attachments`. Errors are surfaced
  // inline (auto-clear on the next add or successful send) rather than
  // thrown — a noisy attach UX is worse than a silent retry.

  function readAsDataUrl(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const fr = new FileReader();
      fr.onload = () => resolve(String(fr.result));
      fr.onerror = () => reject(fr.error ?? new Error('read failed'));
      fr.readAsDataURL(file);
    });
  }

  async function addFiles(files: FileList | File[] | null) {
    if (!files) return;
    const list = Array.from(files);
    if (list.length === 0) return;
    attachError = null;
    // Read every file first, then publish one local batch so a paste
    // of multiple screenshots only rerenders the attachment strip
    // once.
    const accepted: ComposerAttachment[] = [];
    // Snapshot once so the cap check counts already-staged entries
    // without racing the subscription's mirror.
    let staged = attachments.length;
    for (const file of list) {
      if (staged + accepted.length >= MAX_ATTACHMENTS) {
        attachError = `Up to ${MAX_ATTACHMENTS} images per message.`;
        break;
      }
      if (!ACCEPTED_MIME.test(file.type)) {
        attachError = `Skipped ${file.name || 'attachment'}: only image files supported.`;
        continue;
      }
      if (file.size > MAX_ATTACHMENT_BYTES) {
        attachError = `Skipped ${file.name || 'attachment'}: larger than ${Math.round(MAX_ATTACHMENT_BYTES / (1024 * 1024))} MB.`;
        continue;
      }
      try {
        const dataUrl = await readAsDataUrl(file);
        accepted.push({ id: crypto.randomUUID(), name: file.name || 'image', dataUrl });
      } catch (err) {
        console.warn('room: failed to read attachment', err);
        attachError = 'Could not read attachment.';
      }
    }
    if (accepted.length > 0) composerImages.add(accepted);
  }

  function removeAttachment(id: string) {
    composerImages.remove(id);
  }

  function onAttachClick() {
    fileInput?.click();
  }

  function onFilePicked(e: Event) {
    const target = e.currentTarget as HTMLInputElement;
    void addFiles(target.files);
    // Reset so picking the same file twice in a row still fires change.
    target.value = '';
  }

  function onPaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    const files: File[] = [];
    for (const item of items) {
      if (item.kind === 'file') {
        const f = item.getAsFile();
        if (f) files.push(f);
      }
    }
    if (files.length > 0) {
      // Image paste fires alongside whatever text was on the clipboard.
      // Suppress the default so we don't end up with garbled text plus
      // the picture; the user can paste text in a separate gesture.
      e.preventDefault();
      void addFiles(files);
    }
  }

  // True when the drag carries OS files (vs e.g. dragged text from
  // another part of the page). Gates every drag handler so non-file
  // drags pass through to the browser's default behavior unchanged.
  const isFileDrag = (e: DragEvent) =>
    e.dataTransfer?.types.includes('Files') ?? false;

  function onDragEnter(e: DragEvent) {
    if (!isFileDrag(e)) return;
    e.preventDefault();
    dragActive = true;
  }

  function onDragOver(e: DragEvent) {
    if (!isFileDrag(e)) return;
    e.preventDefault();
    e.dataTransfer!.dropEffect = 'copy';
  }

  function onDragLeave(e: DragEvent) {
    // dragleave fires for every child boundary; gate on a real exit by
    // ignoring events where the relatedTarget is still inside the box.
    const wrap = e.currentTarget as HTMLElement;
    if (e.relatedTarget instanceof Node && wrap.contains(e.relatedTarget))
      return;
    dragActive = false;
  }

  function onDrop(e: DragEvent) {
    if (!isFileDrag(e)) return;
    e.preventDefault();
    dragActive = false;
    void addFiles(e.dataTransfer?.files ?? null);
  }

  // -- Input + send --------------------------------------------------

  function onInput() {
    // text was just updated by bind:value with the user's keystroke.
    // Push it into the shared LoroText; Loro diffs against the
    // current state and emits the minimal insert/delete ops.
    composer.update(text);
    cursor = textarea?.selectionStart ?? text.length;
    setTyping(true);
    if (isDraft(threadId)) updateDraftText(threadId, text);
    autosize();
  }

  // Caret moves count as activity once we're already typing, so
  // peers see the cursor track. Don't promote a stray click into a
  // typing session of its own.
  function onCursorMove() {
    cursor = textarea?.selectionStart ?? text.length;
    if (!typing) return;
    scheduleFlush();
  }

  async function send() {
    const body = text.trim();
    // Snapshot the shared list *now* so a peer adding/removing during
    // the HTTP round-trip can't change what we just committed to send.
    const stagedAttachments = composerImages.current();
    const stagedImages = stagedAttachments.map((a) => a.dataUrl);
    const stagedMentions = mentions;
    const stagedSkills = skillInvocations;
    if (
      (!body &&
        stagedImages.length === 0 &&
        stagedMentions.length === 0 &&
        stagedSkills.length === 0) ||
      sending
    )
      return;
    const input: ChatInputItem[] = [];
    if (body) input.push({ type: 'text', text: body });
    for (const skill of stagedSkills) {
      input.push({ type: 'skill', name: skill.name, path: skill.path });
    }
    for (const mention of stagedMentions) {
      input.push({ type: 'mention', name: mention.name, path: mention.path });
    }
    for (const url of stagedImages) {
      input.push({ type: 'image', url });
    }

    // Drafts use a local UUID; Codex assigns the real thread id on
    // the first send and the server echoes it back. Continuations
    // already have a real id and stream under it.
    const fromDraft = isDraft(threadId);
    const fromThreadId = threadId;

    // Commit every visible surface synchronously. composer.update('')
    // clears the shared LoroText and composerImages.clear() empties
    // the local attachment list. The optimistic message lands in the
    // transcript immediately; the server's confirmed copy will
    // replace it transparently via the dedup branch in
    // currentRoom.appendMessage.
    sending = true;
    composer.update('');
    text = '';
    composerImages.clear();
    mentions = [];
    skillInvocations = [];
    attachError = null;
    setTyping(false);
    autosize();
    const optimisticId = currentRoom.appendOptimisticUserMessage(
      fromThreadId,
      body,
      stagedImages
    );

    try {
      const resp = await sendChat({
        serverId,
        thread_id: fromDraft ? null : fromThreadId,
        text: body,
        author: self,
        images: stagedImages,
        input,
        model: selectedModel || undefined,
        effort: selectedEffort || undefined,
        approval_policy: selectedApproval || undefined
      });
      if (fromDraft) {
        discardDraft(fromThreadId);
        if (resp.thread_id !== fromThreadId) {
          // The optimistic lives under the draft id; the server's
          // confirmed copy will land under the real id. The draft
          // bucket is orphaned after the route, so no cleanup
          // needed.
          router.go(
            '/s/' + encodeURIComponent(serverId) + '/t/' + encodeURIComponent(resp.thread_id)
          );
        }
      }
    } catch (err) {
      console.warn('room: send failed', err);
      currentRoom.dropOptimisticMessage(fromThreadId, optimisticId);
      // Restore the body and attachments so the user can retry, but
      // only if they have not started typing or attaching the next
      // message yet. Replacing fresh local intent with a failed retry
      // is worse than losing the failed write.
      if (text === '' && composerImages.current().length === 0) {
        composer.update(body);
        text = body;
        composerImages.add(stagedAttachments);
        mentions = stagedMentions;
        skillInvocations = stagedSkills;
        autosize();
      }
    } finally {
      sending = false;
    }
  }

  function onKey(e: KeyboardEvent) {
    if (e.key === 'Enter' && !e.shiftKey) {
      if (slashMatches.length > 0) {
        e.preventDefault();
        runSlashCommand(slashMatches[0]);
        return;
      }
      if (fileResults.length > 0 && mentionToken) {
        e.preventDefault();
        addMention(fileResults[0]);
        return;
      }
      e.preventDefault();
      void send();
      return;
    }
    if (e.key === 'Escape') {
      if (slashMatches.length > 0 || fileResults.length > 0) {
        fileResults = [];
        return;
      }
      if (modelPickerOpen || approvalPickerOpen) {
        closePickers();
        return;
      }
      // Hand the keyboard to the transcript so j/k/g/G/⌃u/⌃d start
      // scrolling. ThreadDetail's window keydown handler is gated
      // on no text field being focused, so blur() is enough.
      e.preventDefault();
      textarea?.blur();
    }
  }

</script>

<svelte:window onpointerdown={onWindowPointerDown} />

<div class="composer">
  {#if slashMatches.length > 0}
    <div class="palette command-palette" role="listbox" aria-label="Slash commands">
      {#each slashMatches as command (command.name)}
        {@const CommandIcon = command.icon}
        <button type="button" onclick={() => runSlashCommand(command)}>
          <CommandIcon class="palette-icon" />
          <span class="palette-main">/{command.name}</span>
          <span class="palette-sub">{command.description}</span>
        </button>
      {/each}
    </div>
  {:else if mentionToken && (fileResults.length > 0 || fileSearchError)}
    <div class="palette file-palette" role="listbox" aria-label="File mentions">
      {#if fileSearchError}
        <p>{fileSearchError}</p>
      {:else}
        {#each fileResults as result (result.root + result.path)}
          <button type="button" onclick={() => addMention(result)} title={result.path}>
            <IconFile class="palette-icon" />
            <span class="palette-main">{result.file_name || result.path}</span>
            <span class="palette-sub">{result.path}</span>
          </button>
        {/each}
      {/if}
    </div>
  {/if}
  <div
    class="box"
    class:focused-typing={otherTyping.length > 0}
    class:drag-active={dragActive}
    ondragenter={onDragEnter}
    ondragover={onDragOver}
    ondragleave={onDragLeave}
    ondrop={onDrop}
    role="presentation"
  >
    {#if attachments.length > 0}
      <div class="attachments">
        {#each attachments as a (a.id)}
          <div class="attachment" title={a.name}>
            <img src={a.dataUrl} alt={a.name} />
            <button
              type="button"
              class="remove"
              aria-label="Remove attachment"
              onclick={() => removeAttachment(a.id)}
            >
              <IconX class="action-icon" />
            </button>
          </div>
        {/each}
      </div>
    {/if}
    {#if attachError}
      <div class="attach-error" role="status">{attachError}</div>
    {/if}
    {#if mentions.length > 0 || skillInvocations.length > 0}
      <div class="mentions" aria-label="Structured context">
        {#each skillInvocations as skill (skill.id)}
          <span class="mention-chip" title={skill.path}>
            <IconLightning class="chip-icon" />
            /{skill.name}
            <button type="button" aria-label="Remove skill" onclick={() => removeSkill(skill.id)}>
              <IconX class="chip-x" />
            </button>
          </span>
        {/each}
        {#each mentions as mention (mention.id)}
          <span class="mention-chip" title={mention.path}>
            <IconFile class="chip-icon" />
            {mention.path}
            <button type="button" aria-label="Remove mention" onclick={() => removeMention(mention.id)}>
              <IconX class="chip-x" />
            </button>
          </span>
        {/each}
      </div>
    {/if}

    <div class="textarea-wrap">
      <textarea
        bind:this={textarea}
        bind:value={text}
        oninput={onInput}
        onkeydown={onKey}
        onkeyup={onCursorMove}
        onclick={onCursorMove}
        onpaste={onPaste}
        placeholder="Ask for follow-up changes"
        rows={1}
        spellcheck="false"
      ></textarea>

      <!-- Peer cursors absolutely positioned over the textarea at
           the coordinates returned by the mirror-div. Each carries a
           small avatar pill anchored above so it's clear who is
           editing where. -->
      {#each peerCarets as c (c.id)}
        <span
          class="peer-caret"
          title={c.name}
          style="
            top: {c.top}px;
            left: {c.left}px;
            height: {c.height}px;
            --peer-color: {c.color};
          "
        >
          <span class="peer-avatar">
            <Avatar name={c.name} github={c.github} size={16} />
          </span>
        </span>
      {/each}
    </div>

    <div class="actions">
      <button
        class="ghost"
        type="button"
        title="Attach image"
        aria-label="Attach image"
        onclick={onAttachClick}
      >
        <IconPlus class="action-icon" />
      </button>
      <input
        bind:this={fileInput}
        type="file"
        accept="image/*"
        multiple
        hidden
        onchange={onFilePicked}
      />

      <span class="spacer"></span>

      <div class="picker-wrap model-picker-wrap" bind:this={modelPickerRoot}>
        <button
          class="pill-control model-trigger"
          type="button"
          aria-haspopup="listbox"
          aria-expanded={modelPickerOpen}
          title="Model and reasoning effort"
          onclick={toggleModelPicker}
        >
          <IconLightning class="control-icon" />
          <span>5.5</span>
          <span class="muted">{effortLabel(selectedEffort)}</span>
        </button>
        {#if modelPickerOpen}
          <PickerMenu className="model-menu" role="listbox" width="168px" ariaLabel="Reasoning effort">
            {#each FALLBACK_EFFORTS as effort (effort)}
              <PickerMenuItem
                selected={effort === selectedEffort}
                role="option"
                ariaChecked={effort === selectedEffort}
                onclick={() => selectEffort(effort)}
              >
                <span class="picker-item-label">{effortLabel(effort)}</span>
                {#if effort === selectedEffort}
                  <IconCheck class="picker-item-check" />
                {/if}
              </PickerMenuItem>
            {/each}
          </PickerMenu>
        {/if}
      </div>
      <div class="picker-wrap" bind:this={approvalPickerRoot}>
        <button
          class="pill-control"
          type="button"
          aria-haspopup="listbox"
          aria-expanded={approvalPickerOpen}
          title="Approval policy"
          onclick={toggleApprovalPicker}
        >
          <IconShieldCheck class="control-icon" />
          <span>{selectedApprovalLabel}</span>
        </button>
        {#if approvalPickerOpen}
          <PickerMenu className="approval-menu" role="listbox" width="124px" ariaLabel="Approval policy">
            {#each APPROVAL_OPTIONS as option (option.value)}
              <PickerMenuItem
                selected={option.value === selectedApproval}
                role="option"
                ariaChecked={option.value === selectedApproval}
                onclick={() => selectApproval(option.value)}
              >
                <span class="picker-item-label">{option.label}</span>
              </PickerMenuItem>
            {/each}
          </PickerMenu>
        {/if}
      </div>

      <span class="hint" class:ready={canSend}>
        <kbd>↵</kbd> send
        <span class="sep">·</span>
        <kbd>⇧↵</kbd> newline
      </span>
    </div>
  </div>
</div>

<style>
  .composer {
    /* Bottom padding has to clear the box's focus-within shadow
       (which extends ~18px below the pill) AND leave visible
       blank pane bg before the StatusBar — otherwise the shadow's
       diffuse edge runs into the bar and the pill looks pasted
       onto it. 32px gives a clean band even at the strongest
       focus state. */
    padding: 10px 22px 32px;
    background: transparent;
    position: relative;
  }

  .palette {
    position: absolute;
    left: 34px;
    right: 34px;
    bottom: calc(100% - 10px);
    z-index: 20;
    max-height: 240px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: 10px;
    background: var(--bg-elev);
    box-shadow: 0 12px 30px rgba(0, 0, 0, 0.28);
    padding: 6px;
  }
  .palette button {
    width: 100%;
    min-width: 0;
    display: grid;
    grid-template-columns: 18px minmax(90px, 0.45fr) 1fr;
    align-items: center;
    gap: 8px;
    padding: 7px 8px;
    border-radius: 7px;
    color: var(--text-muted);
    text-align: left;
  }
  .palette button:hover,
  .palette button:focus-visible {
    background: var(--bg-active);
    color: var(--text-strong);
  }
  :global(.palette-icon) {
    width: 14px;
    height: 14px;
    color: var(--text-dim);
  }
  .palette-main {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--text-strong);
    font-size: 12px;
  }
  .palette-sub {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    font-size: 11.5px;
    color: var(--text-dim);
  }
  .palette p {
    margin: 0;
    padding: 8px 10px;
    color: var(--danger);
    font-size: 12px;
  }

  /* The textarea-wrap is the positioning root for peer carets — they
     sit absolutely over the textarea at coordinates resolved by the
     mirror-div in lib/caretPos.ts. */
  .textarea-wrap {
    position: relative;
  }

  /* Peer caret. Figma / Linear / Notion style:
       - Steady, no blink.
       - NO position transition. The shared text updates instantly
         on each broadcast (a new char appears at the end of the
         line); animating the caret's top/left between samples just
         makes the cursor visually lag behind its own text. Snap.
       - Slim 1.5px bar; the avatar pill anchors at the top. */
  .peer-caret {
    position: absolute;
    width: 1.5px;
    background: var(--peer-color);
    border-radius: 1px;
    pointer-events: none;
    z-index: 2;
  }
  /* Figma-style: small avatar circle anchored to the top of the
     caret, ringed in the peer's color. Recognizable at a glance,
     less verbose than a name pill — name is in the title for
     hover-tooltip access. */
  .peer-avatar {
    position: absolute;
    bottom: 100%;
    left: -7px;
    margin-bottom: 2px;
    display: inline-flex;
    border-radius: 999px;
    box-shadow:
      0 0 0 1.5px var(--peer-color),
      0 1px 2px rgba(0, 0, 0, 0.18);
    pointer-events: none;
  }

  .box {
    display: flex;
    flex-direction: column;
    gap: 6px;
    background: var(--bg-elev);
    border-radius: var(--radius-xl);
    padding: 12px 14px 10px;
    box-shadow: var(--shadow-inset),
                0 1px 2px rgba(0, 0, 0, 0.04),
                0 3px 10px rgba(0, 0, 0, 0.04);
    transition: box-shadow 0.15s ease;
  }
  /* Tightened from a longer drop so the focused pill ends visibly
     above the StatusBar instead of fading into it. */
  .box:focus-within {
    box-shadow: 0 0 0 0.5px var(--border-hi),
                0 1px 2px rgba(0, 0, 0, 0.05),
                0 4px 12px rgba(0, 0, 0, 0.06);
  }
  /* Drag highlight reuses the focus ring so the affordance reads as
     "this is the drop target" without introducing a new color token. */
  .box.drag-active {
    box-shadow: 0 0 0 1.5px var(--border-hi),
                0 1px 2px rgba(0, 0, 0, 0.05),
                0 4px 12px rgba(0, 0, 0, 0.06);
  }

  .attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding-bottom: 2px;
  }
  .attachment {
    position: relative;
    width: 56px;
    height: 56px;
    border-radius: 8px;
    overflow: hidden;
    background: var(--bg-pill);
    line-height: 0;
  }
  .attachment img {
    width: 100%;
    height: 100%;
    object-fit: cover;
    display: block;
  }
  .attachment .remove {
    position: absolute;
    top: 2px;
    right: 2px;
    width: 18px;
    height: 18px;
    border-radius: 999px;
    background: rgba(0, 0, 0, 0.6);
    color: #fff;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    opacity: 0;
    transition: opacity 0.12s;
  }
  .attachment:hover .remove,
  .attachment .remove:focus-visible {
    opacity: 1;
  }
  .attachment .remove :global(.action-icon) {
    width: 11px;
    height: 11px;
    stroke-width: 2;
  }

  .attach-error {
    color: var(--accent-warn, #c87b00);
    font-size: 11.5px;
    line-height: 1.4;
  }

  .mentions {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
    padding-bottom: 2px;
  }
  .mention-chip {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    max-width: 100%;
    min-width: 0;
    height: 24px;
    padding: 0 6px;
    border-radius: 6px;
    background: var(--bg-pill);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: 11.5px;
    white-space: nowrap;
  }
  .mention-chip :global(.chip-icon) {
    width: 13px;
    height: 13px;
    flex-shrink: 0;
  }
  .mention-chip button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 16px;
    height: 16px;
    border-radius: 4px;
    color: var(--text-dim);
  }
  .mention-chip button:hover {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  :global(.chip-x) {
    width: 11px;
    height: 11px;
  }

  textarea {
    width: 100%;
    min-height: 24px;
    max-height: 50vh;
    resize: none;
    border: none;
    outline: none;
    background: transparent;
    color: var(--text);
    font-size: 14px;
    line-height: 1.55;
    font-family: inherit;
    padding: 0;
    overflow-y: auto;
  }
  textarea::placeholder {
    color: var(--text-dim);
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--text-dim);
    font-size: 11.5px;
  }
  .picker-wrap {
    position: relative;
    display: inline-flex;
    align-items: center;
  }
  .pill-control {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    height: 24px;
    max-width: 210px;
    min-width: 0;
    padding: 0 7px;
    border-radius: 6px;
    background: transparent;
    color: var(--text-muted);
    font-size: 11.5px;
    cursor: pointer;
  }
  .pill-control:hover,
  .pill-control[aria-expanded='true'] {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  .pill-control span {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .pill-control .muted {
    color: var(--text-dim);
  }
  :global(.control-icon) {
    width: 14px;
    height: 14px;
    flex-shrink: 0;
    color: var(--text-dim);
  }
  :global(.model-menu) {
    position: absolute;
    right: 0;
    bottom: 31px;
    z-index: 30;
  }
  :global(.approval-menu) {
    position: absolute;
    right: 0;
    bottom: 31px;
    z-index: 30;
  }
  .spacer {
    flex: 1;
  }
  .ghost {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 24px;
    height: 24px;
    border-radius: 6px;
    color: var(--text-muted);
    transition: background 0.12s, color 0.12s;
  }
  .ghost :global(.action-icon) {
    width: 14px;
    height: 14px;
    stroke-width: 1.75;
  }
  .ghost:hover:not(:disabled) {
    background: var(--bg-hover);
    color: var(--text-strong);
  }
  .ghost:disabled {
    opacity: 0.55;
    cursor: default;
  }

  .hint {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    color: var(--text-dim);
    font-size: 11px;
    opacity: 0.85;
    transition: color 0.12s, opacity 0.12s;
  }
  .hint.ready {
    color: var(--text-muted);
    opacity: 1;
  }
  .hint kbd {
    font-family: var(--font-sans);
    font-size: 10.5px;
    padding: 1px 5px;
    border-radius: 4px;
    background: var(--bg-pill);
    color: inherit;
  }
  .hint .sep {
    color: var(--text-dim);
  }
</style>
