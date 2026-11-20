import { useCallback, useEffect, useState } from 'react';
import { AppState, Platform } from 'react-native';

import Kitedrop, {
  type Device,
  type Info,
  type Received,
  type Transfer,
} from '../../modules/kitedrop-native/src/KitedropNativeModule';

export { Kitedrop };
export type { Device, Info, Received, Transfer };

export type Snapshot = {
  info: Info | null;
  devices: Device[];
  transfers: Transfer[];
  received: Received[];
  issues: string[];
  error: string | null;
};

const empty: Snapshot = { info: null, devices: [], transfers: [], received: [], issues: [], error: null };

function read(): Snapshot {
  try {
    const info = Kitedrop.start();
    return {
      info,
      devices: Kitedrop.devices(),
      transfers: Kitedrop.transfers(50),
      received: Kitedrop.received(30),
      issues: Kitedrop.diagnostics(),
      error: null,
    };
  } catch (e) {
    return { ...empty, error: String(e) };
  }
}

/** Live view of the node: re-read on every core event and when the app comes to the front. */
export function useKitedrop(): Snapshot & { reload: () => void } {
  // First snapshot during the first render; later ones on events (no setState inside the effect body).
  const [snap, setSnap] = useState<Snapshot>(read);
  const reload = useCallback(() => setSnap(read()), []);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const sub = Kitedrop.addListener('onChange', () => {
      clearTimeout(timer);
      timer = setTimeout(reload, 150);
    });
    const app = AppState.addEventListener('change', (s) => {
      if (s === 'active') {
        reload();
        Kitedrop.refresh();
        if (Platform.OS === 'ios') pullNow();
      }
    });
    if (Platform.OS === 'ios') pullNow();
    return () => {
      clearTimeout(timer);
      sub.remove();
      app.remove();
    };
  }, [reload]);

  return { ...snap, reload };
}

/** iPhone: fetch items waiting in the other devices' outboxes (design §8). */
export async function pullNow(): Promise<number> {
  try {
    return await Kitedrop.pollOutboxes();
  } catch {
    return 0;
  }
}

export const PLATFORM_NAME: Record<string, string> = {
  ios: 'iPhone',
  android: 'Android',
  macos: 'Mac',
  windows: 'Windows',
};

export function sizeText(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 ** 3) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 ** 3).toFixed(2)} GB`;
}

/** Plain-language failure reasons (design §3: no silent failures). */
export function reasonText(reason: string | null): string {
  if (!reason) return '';
  if (reason === 'offline') return 'Device offline — will retry';
  if (reason === 'expired') return 'Waited too long without reaching the device';
  if (reason === 'cancelled') return 'Cancelled';
  if (reason === 'removed_from_group') return 'Device no longer in your group';
  if (reason === 'file_unreadable') return 'The file could not be read';
  if (reason.startsWith('rejected:')) return `Refused by the device (${reason.slice(9)})`;
  return reason;
}

export const ISSUE_TEXT: Record<string, string> = {
  no_network: 'Not connected to a network.',
  local_network_denied:
    'kitedrop may not use the local network. Allow it in Settings → Privacy & Security → Local Network → kitedrop.',
  different_subnet: 'One of your devices was last seen on a different network.',
  ap_isolation_suspected:
    'Your devices cannot see each other on this Wi-Fi. It may be a guest network or have client isolation on.',
  subnet_too_large: 'This network is very large; kitedrop only searches the nearby part of it.',
  port_in_use: 'kitedrop could not open a network port.',
  windows_public_profile: 'A Windows PC treats this network as Public.',
};

export function looksLikeLink(t: string): boolean {
  const s = t.trim();
  return !/\s/.test(s) && /^https?:\/\//.test(s);
}

/** "just now", "5 min ago", "2 h ago", "3 d ago" for a unix time in seconds. */
export function ago(unixSecs: number): string {
  const s = Math.max(0, Math.floor(Date.now() / 1000 - unixSecs));
  if (s < 60) return 'just now';
  if (s < 3600) return `${Math.floor(s / 60)} min ago`;
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`;
  return `${Math.floor(s / 86400)} d ago`;
}

/** Badge text and color for a sent item. */
export function stateBadge(t: Transfer): { label: string; tone: 'ok' | 'err' | 'accent' | 'muted' } {
  switch (t.state) {
    case 'delivered':
      return { label: 'Delivered', tone: 'ok' };
    case 'failed':
      return { label: t.reason === 'cancelled' ? 'Cancelled' : 'Failed', tone: t.reason === 'cancelled' ? 'muted' : 'err' };
    case 'waiting':
      return { label: t.viaOutbox ? 'Waiting for iPhone' : 'Waiting', tone: 'accent' };
    case 'sending':
      return { label: t.progress ? `Sending ${percent(t)}%` : 'Sending', tone: 'accent' };
    default:
      return {
        label: t.progress?.phase === 'sealing' ? `Encrypting ${percent(t)}%` : 'Queued',
        tone: 'accent',
      };
  }
}

export function itemTitle(x: { name: string; kind: 'file' | 'text' | 'link' }): string {
  return x.name || (x.kind === 'link' ? 'Link' : 'Text');
}

export function percent(t: Transfer): number {
  const p = t.progress;
  return p && p.total > 0 ? Math.min(100, Math.floor((p.done * 100) / p.total)) : 0;
}

/** "280 MB of 799 MB" for a send in progress. */
export function progressText(t: Transfer): string | null {
  return t.progress ? `${sizeText(t.progress.done)} of ${sizeText(t.progress.total)}` : null;
}
