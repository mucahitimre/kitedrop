import { SymbolView, type AndroidSymbol, type SFSymbol } from 'expo-symbols';
import { Children, type ReactNode } from 'react';
import {
  Pressable,
  StyleSheet,
  Text,
  TextInput,
  View,
  type TextInputProps,
  type ViewStyle,
} from 'react-native';

import { useTheme } from '@/lib/theme';

/** SF Symbols on iOS, Material Symbols on Android (expo-symbols), so each platform looks native. */
const ICONS = {
  add: { ios: 'plus', android: 'add' },
  history: { ios: 'clock.arrow.circlepath', android: 'history' },
  clipboard: { ios: 'doc.on.clipboard', android: 'content_paste' },
  text: { ios: 'text.alignleft', android: 'notes' },
  file: { ios: 'doc', android: 'description' },
  link: { ios: 'link', android: 'link' },
  attach: { ios: 'paperclip', android: 'attach_file' },
  photo: { ios: 'photo', android: 'image' },
  chevron: { ios: 'chevron.right', android: 'chevron_right' },
  warning: { ios: 'exclamationmark.triangle.fill', android: 'warning' },
  check: { ios: 'checkmark.circle.fill', android: 'check_circle' },
  error: { ios: 'xmark.octagon.fill', android: 'error' },
  scan: { ios: 'qrcode.viewfinder', android: 'qr_code_scanner' },
  qr: { ios: 'qrcode', android: 'qr_code_2' },
  send: { ios: 'paperplane.fill', android: 'send' },
  trash: { ios: 'trash', android: 'delete' },
  battery: { ios: 'battery.100.bolt', android: 'battery_charging_full' },
  pair: { ios: 'link.badge.plus', android: 'add_link' },
  ios: { ios: 'iphone', android: 'phone_iphone' },
  android: { ios: 'smartphone', android: 'smartphone' },
  macos: { ios: 'laptopcomputer', android: 'laptop_mac' },
  windows: { ios: 'desktopcomputer', android: 'desktop_windows' },
} satisfies Record<string, { ios: SFSymbol; android: AndroidSymbol }>;

export type IconName = keyof typeof ICONS;

export function Icon({ name, size = 20, color }: { name: IconName; size?: number; color?: string }) {
  const t = useTheme();
  const n = ICONS[name];
  return <SymbolView name={{ ...n, web: n.android }} size={size} tintColor={color ?? t.text} />;
}

/** App mark: same kite shape as the desktop logo. */
export function Logo({ size = 40 }: { size?: number }) {
  const t = useTheme();
  const k = size * 0.36;
  return (
    <View style={[styles.logo, { width: size, height: size, borderRadius: size * 0.27, backgroundColor: t.accent }]}>
      <View style={{ width: k, height: k, backgroundColor: '#fff', transform: [{ scaleY: 1.25 }, { rotate: '45deg' }], borderRadius: 2 }} />
    </View>
  );
}

export function Card({ children, style }: { children: ReactNode; style?: ViewStyle }) {
  const t = useTheme();
  return <View style={[styles.card, { backgroundColor: t.card, borderColor: t.line }, style]}>{children}</View>;
}

type Variant = 'primary' | 'secondary' | 'danger';

export function Button({
  title,
  onPress,
  primary,
  variant,
  icon,
  disabled,
  flex,
}: {
  title: string;
  onPress: () => void;
  /** Shorthand for `variant="primary"`. */
  primary?: boolean;
  variant?: Variant;
  icon?: IconName;
  disabled?: boolean;
  /** Share a row equally with sibling buttons. */
  flex?: boolean;
}) {
  const t = useTheme();
  const v: Variant = variant ?? (primary ? 'primary' : 'secondary');
  const fg = v === 'primary' ? t.onAccent : v === 'danger' ? t.err : t.text;
  const bg = v === 'primary' ? t.accent : v === 'danger' ? t.errSoft : t.card;
  const border = v === 'primary' ? t.accent : v === 'danger' ? 'transparent' : t.line;
  return (
    <Pressable
      onPress={onPress}
      disabled={disabled}
      accessibilityRole="button"
      style={({ pressed }) => [
        styles.button,
        { borderColor: border, backgroundColor: bg },
        flex && { flex: 1 },
        (pressed || disabled) && { opacity: disabled ? 0.45 : 0.7 },
      ]}
    >
      {icon && <Icon name={icon} size={16} color={fg} />}
      <Text style={[styles.buttonText, { color: fg }]} numberOfLines={1}>
        {title}
      </Text>
    </Pressable>
  );
}

export function IconButton({ icon, label, onPress }: { icon: IconName; label: string; onPress: () => void }) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      accessibilityRole="button"
      accessibilityLabel={label}
      hitSlop={8}
      style={({ pressed }) => [styles.iconButton, pressed && { backgroundColor: t.accentSoft }]}
    >
      <Icon name={icon} size={22} color={t.accent} />
    </Pressable>
  );
}

/** Device platform icon with an online dot (no dot for the iPhone: it is never "online"). */
export function Avatar({ platform, online, size = 40 }: { platform: string; online?: boolean | null; size?: number }) {
  const t = useTheme();
  const name = (platform in ICONS ? platform : 'android') as IconName;
  return (
    <View
      style={[
        styles.avatar,
        { width: size, height: size, borderRadius: size * 0.27, backgroundColor: t.accentSoft },
      ]}
    >
      <Icon name={name} size={size * 0.5} color={t.accent} />
      {online != null && (
        <View style={[styles.statusDot, { backgroundColor: online ? t.ok : t.muted, borderColor: t.card }]} />
      )}
    </View>
  );
}

/** Small round icon tile for list rows (file / text / link). */
export function KindIcon({ kind }: { kind: IconName }) {
  const t = useTheme();
  return (
    <View style={[styles.kind, { backgroundColor: t.bg }]}>
      <Icon name={kind} size={16} color={t.muted} />
    </View>
  );
}

/** Thin bar for a send in progress (0…100). */
export function ProgressBar({ percent, muted }: { percent: number; muted?: boolean }) {
  const t = useTheme();
  return (
    <View style={[styles.progress, { backgroundColor: t.line }]}>
      <View style={[styles.progressBar, { width: `${percent}%`, backgroundColor: muted ? t.muted : t.accent }]} />
    </View>
  );
}

export type BadgeTone = 'ok' | 'err' | 'accent' | 'muted';

export function Badge({ label, tone }: { label: string; tone: BadgeTone }) {
  const t = useTheme();
  const colors = {
    ok: [t.okSoft, t.ok],
    err: [t.errSoft, t.err],
    accent: [t.accentSoft, t.accent],
    muted: [t.bg, t.muted],
  }[tone];
  return (
    <View style={[styles.badge, { backgroundColor: colors[0] }]}>
      <Text style={[styles.badgeText, { color: colors[1] }]}>{label}</Text>
    </View>
  );
}

export function Heading({ children, action }: { children: ReactNode; action?: ReactNode }) {
  const t = useTheme();
  return (
    <View style={styles.headingRow}>
      <Text style={[styles.heading, { color: t.muted }]}>{children}</Text>
      {action}
    </View>
  );
}

export function Banner({ lines }: { lines: string[] }) {
  const t = useTheme();
  if (lines.length === 0) return null;
  return (
    <View style={[styles.banner, { backgroundColor: t.warnBg }]}>
      <Icon name="warning" size={18} color={t.warnText} />
      <View style={{ flex: 1, gap: 4 }}>
        {lines.map((l) => (
          <Text key={l} style={{ color: t.warnText, lineHeight: 20 }}>
            {l}
          </Text>
        ))}
      </View>
    </View>
  );
}

/** Success / error line after an action. */
export function Notice({ ok, text }: { ok: boolean; text: string }) {
  const t = useTheme();
  return (
    <View style={[styles.notice, { backgroundColor: ok ? t.okSoft : t.errSoft }]}>
      <Icon name={ok ? 'check' : 'error'} size={18} color={ok ? t.ok : t.err} />
      <Text style={{ color: ok ? t.ok : t.err, flex: 1, fontWeight: '500' }}>{text}</Text>
    </View>
  );
}

/** Rows inside one rounded box, separated by hairlines (iOS grouped-list style). */
export function Group({ children }: { children: ReactNode }) {
  const t = useTheme();
  const items = Children.toArray(children).filter(Boolean);
  if (items.length === 0) return null;
  return (
    <View style={[styles.group, { backgroundColor: t.card, borderColor: t.line }]}>
      {items.map((c, i) => (
        <View key={i} style={i > 0 && { borderTopWidth: StyleSheet.hairlineWidth, borderTopColor: t.line }}>
          {c}
        </View>
      ))}
    </View>
  );
}

export function Row({
  left,
  title,
  sub,
  right,
  below,
  onPress,
}: {
  left?: ReactNode;
  title: string;
  sub?: string;
  right?: ReactNode;
  /** Extra content under the subtitle (progress bar). */
  below?: ReactNode;
  onPress?: () => void;
}) {
  const t = useTheme();
  return (
    <Pressable
      onPress={onPress}
      disabled={!onPress}
      style={({ pressed }) => [styles.row, pressed && { backgroundColor: t.bg }]}
    >
      {left}
      <View style={{ flex: 1, minWidth: 0 }}>
        <Text style={[styles.rowTitle, { color: t.text }]} numberOfLines={1}>
          {title}
        </Text>
        {sub ? (
          <Text style={{ color: t.muted, fontSize: 13 }} numberOfLines={2}>
            {sub}
          </Text>
        ) : null}
        {below}
      </View>
      {right}
    </Pressable>
  );
}

export function Segmented<T extends string>({
  options,
  value,
  onChange,
}: {
  options: { value: T; label: string; icon?: IconName }[];
  value: T;
  onChange: (v: T) => void;
}) {
  const t = useTheme();
  return (
    <View style={[styles.segmented, { backgroundColor: t.card, borderColor: t.line }]}>
      {options.map((o) => {
        const active = o.value === value;
        const fg = active ? t.onAccent : t.muted;
        return (
          <Pressable
            key={o.value}
            onPress={() => onChange(o.value)}
            accessibilityRole="tab"
            accessibilityState={{ selected: active }}
            style={[styles.seg, active && { backgroundColor: t.accent }]}
          >
            {o.icon && <Icon name={o.icon} size={16} color={fg} />}
            <Text style={{ color: fg, fontWeight: '600' }} numberOfLines={1}>
              {o.label}
            </Text>
          </Pressable>
        );
      })}
    </View>
  );
}

export function Field(props: TextInputProps) {
  const t = useTheme();
  return (
    <TextInput
      placeholderTextColor={t.muted}
      {...props}
      style={[styles.field, { color: t.text, borderColor: t.line, backgroundColor: t.card }, props.style]}
    />
  );
}

/** QR code from the core's module matrix; dark runs per row are merged to keep views few. */
export function Qr({ size, modules, width = 240 }: { size: number; modules: number[]; width?: number }) {
  const quiet = 4;
  const cell = width / (size + quiet * 2);
  const rows = [];
  for (let y = 0; y < size; y++) {
    const runs = [];
    let x = 0;
    while (x < size) {
      if (modules[y * size + x] === 1) {
        const start = x;
        while (x < size && modules[y * size + x] === 1) x++;
        runs.push(
          <View
            key={start}
            style={{ position: 'absolute', left: (start + quiet) * cell, width: (x - start) * cell, height: cell, backgroundColor: '#000' }}
          />,
        );
      } else {
        x++;
      }
    }
    rows.push(
      <View key={y} style={{ position: 'absolute', top: (y + quiet) * cell, left: 0, right: 0, height: cell }}>
        {runs}
      </View>,
    );
  }
  return <View style={{ width, height: width, backgroundColor: '#fff', alignSelf: 'center', borderRadius: 12 }}>{rows}</View>;
}

const styles = StyleSheet.create({
  card: { borderRadius: 16, borderWidth: StyleSheet.hairlineWidth, padding: 14, marginBottom: 10 },
  button: {
    borderWidth: 1,
    borderRadius: 12,
    paddingVertical: 11,
    paddingHorizontal: 14,
    alignItems: 'center',
    justifyContent: 'center',
    flexDirection: 'row',
    gap: 6,
  },
  buttonText: { fontWeight: '600', fontSize: 15 },
  iconButton: { width: 36, height: 36, borderRadius: 18, alignItems: 'center', justifyContent: 'center' },
  logo: { alignItems: 'center', justifyContent: 'center' },
  avatar: { alignItems: 'center', justifyContent: 'center' },
  statusDot: { position: 'absolute', right: -2, bottom: -2, width: 13, height: 13, borderRadius: 7, borderWidth: 2 },
  kind: { width: 32, height: 32, borderRadius: 9, alignItems: 'center', justifyContent: 'center' },
  badge: { borderRadius: 999, paddingHorizontal: 9, paddingVertical: 3 },
  badgeText: { fontSize: 12, fontWeight: '600' },
  headingRow: { flexDirection: 'row', justifyContent: 'space-between', alignItems: 'center', marginTop: 22, marginBottom: 8 },
  heading: { fontSize: 12.5, fontWeight: '600', textTransform: 'uppercase', letterSpacing: 0.6 },
  banner: { borderRadius: 14, padding: 12, marginBottom: 10, flexDirection: 'row', gap: 10, alignItems: 'flex-start' },
  notice: { borderRadius: 12, padding: 12, marginTop: 14, flexDirection: 'row', gap: 8, alignItems: 'center' },
  group: { borderRadius: 16, borderWidth: StyleSheet.hairlineWidth, overflow: 'hidden' },
  row: { flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 11, paddingHorizontal: 14 },
  rowTitle: { fontSize: 15.5, fontWeight: '600' },
  progress: { height: 4, borderRadius: 2, overflow: 'hidden', marginTop: 6 },
  progressBar: { height: 4, borderRadius: 2 },
  segmented: { flexDirection: 'row', borderRadius: 12, borderWidth: StyleSheet.hairlineWidth, padding: 3, marginBottom: 14 },
  seg: { flex: 1, flexDirection: 'row', gap: 6, alignItems: 'center', justifyContent: 'center', paddingVertical: 8, borderRadius: 9 },
  field: { borderWidth: 1, borderRadius: 12, padding: 12, fontSize: 15, marginBottom: 10 },
});
