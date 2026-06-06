import { writable } from 'svelte/store';

export type ThreadPanelTab = 'review' | 'files';

export const rightPanelOpen = writable(false);
export const rightPanelTab = writable<ThreadPanelTab>('review');

export function openThreadPanel(tab: ThreadPanelTab): void {
  rightPanelTab.set(tab);
  rightPanelOpen.set(true);
}

export function closeThreadPanel(): void {
  rightPanelOpen.set(false);
}

export function toggleThreadPanel(tab: ThreadPanelTab): void {
  rightPanelTab.set(tab);
  rightPanelOpen.update((open) => !open);
}
