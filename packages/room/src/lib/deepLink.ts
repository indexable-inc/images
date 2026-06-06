// room:// deep link handler.
//
// Tauri's deep-link plugin delivers two flavours of incoming URL:
//   - getCurrent(): URLs the app was *launched* with (the recipient
//     double-clicked the link before Room was running).
//   - onOpenUrl(): URLs delivered while Room is already running
//     (single-instance OS routing on macOS).
//
// Both arrive as `room://t/<thread-id>` from ThreadRow's href. We
// parse the path and hand off to the in-app hash router so the
// existing routing pipeline (sidebar selection, ThreadDetail mount)
// stays the single owner of "which thread is on screen".

import { router } from './router';

function handle(url: string): void {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return;
  }
  if (parsed.protocol !== 'room:') return;
  // room://t/<id> parses as { host: 't', pathname: '/<id>' } on
  // platforms that treat the scheme as authority-bearing. macOS
  // WebKit parses it that way; URL() in modern browsers does too.
  const segments = [parsed.host, ...parsed.pathname.split('/')].filter(
    (s) => s.length > 0
  );
  if (segments[0] !== 't' || !segments[1]) return;
  const threadId = decodeURIComponent(segments[1]);
  router.go('/t/' + encodeURIComponent(threadId));
}

export async function bindDeepLinks(): Promise<() => void> {
  if (typeof window === 'undefined' || !('__TAURI_INTERNALS__' in window)) {
    return () => {};
  }
  const { onOpenUrl, getCurrent } = await import(
    '@tauri-apps/plugin-deep-link'
  );
  try {
    const current = await getCurrent();
    if (current) for (const u of current) handle(u);
  } catch (err) {
    console.warn('room: deep-link getCurrent failed', err);
  }
  try {
    return await onOpenUrl((urls) => {
      for (const u of urls) handle(u);
    });
  } catch (err) {
    console.warn('room: deep-link onOpenUrl failed', err);
    return () => {};
  }
}
