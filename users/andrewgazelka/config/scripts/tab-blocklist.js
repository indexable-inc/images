// tab-blocklist: redirect browser tabs sitting on blocked sites to localhost.
// argv: base domains; the apex and its `www.` host are both blocked, every
// other subdomain is untouched. This is the browser-interaction layer of the
// blocklist: unlike the /etc/hosts sinkhole it blocks only *being on the
// site*, so apps, OAuth flows, and APIs talking to the same domain keep
// working. Polled by the tab-blocklist launchd agent (home/darwin.nix).
//
// Reads every window's tab URLs in ONE batch Apple Event (w.tabs.url()) and
// writes only matching tabs by index. Index access is also the only
// iteration Dia supports: materializing w.tabs() as a JXA array fails there
// with "Can't convert types (-1700)", while indexed specifiers work in all
// three browsers. The delay(0.3) after each write matters too: Dia silently
// drops back-to-back URL writes and only navigates one tab per burst.
// Never launches a browser that is not already running.
'use strict';

function hostOf(url) {
  const m = /^[a-z][a-z0-9+.-]*:\/\/([^/:?#]+)/i.exec(url || '');
  return m ? m[1].toLowerCase() : '';
}

function run(argv) {
  const blocked = new Set();
  for (const d of argv) {
    blocked.add(d.toLowerCase());
    blocked.add('www.' + d.toLowerCase());
  }
  const REDIRECT = 'http://localhost/';
  const hit = [];

  for (const name of ['Safari', 'Google Chrome', 'Dia']) {
    let app;
    try {
      app = Application(name);
      if (!app.running()) continue;
    } catch (e) {
      continue; // not installed
    }
    try {
      const nWin = app.windows.length;
      for (let wi = 0; wi < nWin; wi++) {
        const tabs = app.windows[wi].tabs;
        const urls = tabs.url(); // one Apple Event: every tab URL in this window
        for (let ti = 0; ti < urls.length; ti++) {
          if (blocked.has(hostOf(urls[ti]))) {
            tabs[ti].url = REDIRECT;
            delay(0.3); // Dia drops rapid successive writes
            hit.push(name + ': ' + urls[ti]);
          }
        }
      }
    } catch (e) {
      // browser quit mid-scan; catch and retry next poll
    }
  }
  return hit.join('\n');
}
