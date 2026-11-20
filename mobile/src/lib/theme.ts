import { useColorScheme } from 'react-native';

/** Same palette as the desktop app (desktop/ui/style.css), so both look like one product. */
export type Theme = {
  dark: boolean;
  bg: string;
  card: string;
  text: string;
  muted: string;
  line: string;
  accent: string;
  accentSoft: string;
  onAccent: string;
  ok: string;
  okSoft: string;
  err: string;
  errSoft: string;
  warnBg: string;
  warnText: string;
};

const light: Theme = {
  dark: false,
  bg: '#f6f7fb',
  card: '#ffffff',
  text: '#0f172a',
  muted: '#64748b',
  line: '#e2e8f0',
  accent: '#2563eb',
  accentSoft: '#dbeafe',
  onAccent: '#ffffff',
  ok: '#16a34a',
  okSoft: '#dcfce7',
  err: '#dc2626',
  errSoft: '#fee2e2',
  warnBg: '#fef3c7',
  warnText: '#92400e',
};

const dark: Theme = {
  dark: true,
  bg: '#0b1020',
  card: '#131a2c',
  text: '#e2e8f0',
  muted: '#94a3b8',
  line: '#233049',
  accent: '#3b82f6',
  accentSoft: '#1e3a8a66',
  onAccent: '#ffffff',
  ok: '#22c55e',
  okSoft: '#14532d66',
  err: '#f87171',
  errSoft: '#7f1d1d66',
  warnBg: '#78350f55',
  warnText: '#fde68a',
};

/** Colors for the current light/dark setting (every text has an explicit color). */
export function useTheme(): Theme {
  return useColorScheme() === 'dark' ? dark : light;
}
