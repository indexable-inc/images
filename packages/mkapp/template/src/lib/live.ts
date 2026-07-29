// THE UPDATE SURFACE. Edit this file to change what a reader already has open.
//
// Why here and not in the seed: a page that is already open rehydrated its store
// from sessionStorage on load and will never read the seed again. These
// statements run after that rehydrate, on every promote, so they land on the
// page in front of the reader without a refresh.
//
// Every call is idempotent, so statements can be left in place: this file
// re-runs on each promote, and running it twice must look the same as running it
// once. It also looks the same IN THE HISTORY -- the log records state
// transitions rather than calls, so a re-run with nothing changed appends
// nothing.
//
//   import * as page from './store.svelte.ts';
//
//   // attribute everything after this line; the writer is one agent, but it
//   // relays work from many, and the history should name the one that found it
//   page.by('vrack');
//
//   page.narrate('measuring the CVE gate');
//
//   page.add({
//     id: 'vrack-acl',
//     title: 'The vRack ACL is one rule, and it is missing',
//     loading: false,
//     body: 'Measured on dev-compute-3: 0.14 ms, no filter.',
//   }, 'top');
//
//   page.set('vrack-acl', { loading: true });   // show a skeleton
//   page.say('vrack-acl', 'Reproduced on dev; PR 9102.');   // append once
//   page.remove('vrack-acl');
//   page.narrate('done', true);
//
// ONE SHARP EDGE, worth reading twice. These are mutations, not declarations.
// Deleting a line does NOT undo what it did: the change is already in the
// reader's store. To take a change back, push the inverse -- or call
// `page.reset()` once and remove it afterwards, since leaving it in place would
// wipe every later statement on every promote.
//
// The reader can now see all of this: press H for the history, and click any row
// to see the page as it stood just after that change.
import * as page from './store.svelte.ts';

page.by('mkapp');
page.narrate('waiting for the agent');
