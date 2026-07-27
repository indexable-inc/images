// The version a timestamp names. Shared by the live view and the recording
// replay worker so both time-travel the same way.
//
// A Loro version is a FRONTIER: a SET of op ids, one per peer that has
// contributed to it. The dashboard used to return a single OpId -- the last
// change at or before the requested moment -- which is correct only while the
// aggregator is the sole editor, because then every change causally follows the
// one before it and naming the last one names them all.
//
// That stopped being true the moment a viewer could write. Two viewers answering
// two inputs branch from the same snapshot and never see each other until the hub
// merges them, so their changes are CONCURRENT: neither is in the other's causal
// past. Naming only the later one checks out a version in which the earlier
// answer was never made -- it vanishes from the replay, and from the document the
// user is looking at, with no error. `frontierAt` therefore returns the last op
// of every peer at or before the moment.
//
// (Verified against loro-crdt 1.13.8: with two peers branching off one snapshot
// and answering concurrently, the single-OpId frontier renders one answer and
// drops the other; the per-peer set renders both.)

// One change reduced to what time travel and the edit history need: where its ops
// live in the peer's counter space, and when it was committed.
export interface Mark {
  peer: string;
  // The change's first op counter, and how many ops it holds. Together they are
  // the id span `exportJsonInIdSpan` wants when the edit history asks what a
  // change actually did.
  start: number;
  length: number;
  // The change's LAST op counter (`start + length - 1`): the frontier entry that
  // lands the document just after this change.
  counter: number;
  // Milliseconds since the epoch. The hub stamps every commit with
  // `set_next_commit_timestamp(now_ms())`, and the browser does the same for its
  // own commits, so the whole oplog shares one millisecond axis. (Loro's own
  // default is seconds; nothing here relies on it.)
  ts: number;
  lamport: number;
}

// One entry of a frontier: a peer and the counter of its last included op.
export interface FrontierId {
  peer: string;
  counter: number;
}

// A change's stable identity, matching loro's own `counter@peer` spelling.
export function changeId(peer: string, start: number): string {
  return `${start}@${peer}`;
}

// Fold one `getAllChanges()` entry into a Mark.
export function markOf(
  peer: string,
  change: { counter: number; length: number; timestamp: number; lamport: number },
): Mark {
  return {
    peer,
    start: change.counter,
    length: change.length,
    counter: change.counter + change.length - 1,
    ts: change.timestamp,
    lamport: change.lamport,
  };
}

// Order marks for the timeline: by commit time, then lamport so concurrent
// changes with the same millisecond still have a stable order.
export function sortMarks(marks: Mark[]): Mark[] {
  return marks.sort((a, b) => a.ts - b.ts || a.lamport - b.lamport);
}

// The frontier at or before `ts`: for every peer, its last op committed at or
// before that moment.
//
// Linear in the number of changes rather than a binary search, because the answer
// depends on every peer's history and not just the one nearest mark. That is
// cheap next to the checkout it feeds (which is O(the op distance travelled)),
// and it is the only shape that stays correct with more than one writer.
//
// The result may be non-minimal -- an entry can already be in another entry's
// causal past -- which loro accepts: it resolves the set to a version either way.
export function frontierAt(marks: readonly Mark[], ts: number): FrontierId[] {
  const last = new Map<string, number>();
  for (const mark of marks) {
    if (mark.ts > ts) continue;
    const seen = last.get(mark.peer);
    if (seen === undefined || mark.counter > seen) last.set(mark.peer, mark.counter);
  }
  return [...last].map(([peer, counter]) => ({ peer, counter }));
}
