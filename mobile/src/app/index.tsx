import * as Clipboard from 'expo-clipboard';
import * as DocumentPicker from 'expo-document-picker';
import { router } from 'expo-router';
import { useEffect, useState } from 'react';
import {
  Alert,
  PermissionsAndroid,
  Platform,
  Pressable,
  RefreshControl,
  ScrollView,
  StyleSheet,
  Text,
  View,
} from 'react-native';
import { useSafeAreaInsets } from 'react-native-safe-area-context';

import {
  Avatar,
  Badge,
  Banner,
  Button,
  Card,
  Group,
  Heading,
  Icon,
  IconButton,
  KindIcon,
  Logo,
  ProgressBar,
  Row,
} from '@/components/ui';
import {
  ISSUE_TEXT,
  Kitedrop,
  PLATFORM_NAME,
  ago,
  itemTitle,
  looksLikeLink,
  percent,
  progressText,
  pullNow,
  sizeText,
  stateBadge,
  useKitedrop,
  type Device,
} from '@/lib/kitedrop';
import { useTheme } from '@/lib/theme';

export default function Home() {
  const t = useTheme();
  const insets = useSafeAreaInsets();
  const k = useKitedrop();
  const [pulling, setPulling] = useState(false);

  useEffect(() => {
    if (Platform.OS === 'ios' && k.info) Kitedrop.requestPhotosAccess();
    if (Platform.OS !== 'android') return;
    // Notifications tell the user what arrived and what failed (design §3).
    if (Platform.Version >= 33) {
      PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS);
    }
  }, [k.info]);


  // Read on every render: the screen re-renders when the app returns from the battery settings.
  const batteryOk = Platform.OS !== 'android' || Kitedrop.isIgnoringBatteryOptimizations();
  const others = k.devices.filter((d) => !d.isSelf);
  const name = (id: string) => k.devices.find((d) => d.id === id)?.name ?? 'removed device';
  const issues = [...new Set(k.issues.map((c) => ISSUE_TEXT[c] ?? c))];
  if (k.error) issues.unshift(`kitedrop could not start: ${k.error}`);

  async function refresh() {
    setPulling(true);
    Kitedrop.refresh();
    if (Platform.OS === 'ios') await pullNow();
    k.reload();
    setPulling(false);
  }

  async function sendClipboard(d: Device) {
    const text = await Clipboard.getStringAsync();
    if (!text) return Alert.alert('The clipboard is empty.');
    try {
      if (looksLikeLink(text)) await Kitedrop.sendLink([d.id], text.trim());
      else await Kitedrop.sendText([d.id], text);
    } catch (e) {
      Alert.alert('Could not send', String(e));
    }
  }

  async function sendFiles(d: Device) {
    const r = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
    if (r.canceled) return;
    try {
      for (const a of r.assets) await Kitedrop.sendFile([d.id], a.uri, a.name, a.mimeType ?? null);
    } catch (e) {
      Alert.alert('Could not send', String(e));
    }
  }

  const recent = k.transfers.slice(0, 4);

  return (
    <ScrollView
      contentContainerStyle={[styles.page, { paddingTop: insets.top + 12 }]}
      refreshControl={<RefreshControl refreshing={pulling} onRefresh={refresh} />}
    >
      <View style={styles.topbar}>
        <Logo />
        <View style={{ flex: 1, minWidth: 0 }}>
          <Text style={[styles.brand, { color: t.text }]}>kitedrop</Text>
          {k.info && (
            <Text style={{ color: t.muted, fontSize: 13 }} numberOfLines={1}>
              {k.info.name} · {PLATFORM_NAME[k.info.platform]}
            </Text>
          )}
        </View>
        <IconButton icon="history" label="Activity" onPress={() => router.push('/activity')} />
        <IconButton icon="add" label="Add a device" onPress={() => router.push('/add')} />
      </View>
      <Banner lines={issues} />
      {Platform.OS === 'android' && !batteryOk && (
        <Card style={styles.battery}>
          <Icon name="battery" size={22} color={t.accent} />
          <View style={{ flex: 1, gap: 10 }}>
            <Text style={{ color: t.text, lineHeight: 20 }}>
              To receive while the screen is off, let kitedrop run without battery restrictions.
            </Text>
            <Button title="Allow" primary onPress={() => Kitedrop.requestIgnoreBatteryOptimizations()} />
          </View>
        </Card>
      )}

      <Heading>Your devices</Heading>
      {others.map((d) => (
        <Card key={d.id}>
          <Pressable
            onPress={() => router.push({ pathname: '/device', params: { id: d.id } })}
            style={({ pressed }) => [styles.deviceHead, pressed && { opacity: 0.6 }]}
          >
            <Avatar platform={d.platform} online={d.platform === 'ios' ? null : d.online} />
            <View style={{ flex: 1, minWidth: 0 }}>
              <Text style={[styles.name, { color: t.text }]} numberOfLines={1}>
                {d.name}
              </Text>
              <Text style={{ color: t.muted, fontSize: 13 }} numberOfLines={1}>
                {PLATFORM_NAME[d.platform]} · {status(d)}
              </Text>
            </View>
            <Icon name="chevron" size={14} color={t.muted} />
          </Pressable>
          <View style={styles.actions}>
            <Button title="Clipboard" icon="clipboard" flex onPress={() => sendClipboard(d)} />
            <Button
              title="Text"
              icon="text"
              flex
              onPress={() => router.push({ pathname: '/compose', params: { to: d.id } })}
            />
            <Button title="Files" icon="attach" flex onPress={() => sendFiles(d)} />
          </View>
        </Card>
      ))}
      <Pressable
        onPress={() => router.push('/add')}
        style={({ pressed }) => [styles.add, { borderColor: t.accent }, pressed && { backgroundColor: t.accentSoft }]}
      >
        <Icon name="add" size={18} color={t.accent} />
        <Text style={{ color: t.accent, fontWeight: '600', fontSize: 15 }}>
          {others.length === 0 ? 'Add your first device' : 'Add a device'}
        </Text>
      </Pressable>
      {others.length === 0 && (
        <Text style={[styles.hint, { color: t.muted }]}>
          Pair with your computer or another phone to send text, links and files between them.
        </Text>
      )}

      {recent.length > 0 && (
        <Heading
          action={
            <Pressable onPress={() => router.push('/activity')} hitSlop={8}>
              <Text style={{ color: t.accent, fontWeight: '600' }}>See all</Text>
            </Pressable>
          }
        >
          Recently sent
        </Heading>
      )}
      <Group>
        {recent.map((tr) => {
          const b = stateBadge(tr);
          return (
            <Row
              key={tr.id}
              left={<KindIcon kind={tr.kind} />}
              title={itemTitle(tr)}
              sub={(progressText(tr)
                ? [`to ${name(tr.targetId)}`, progressText(tr)]
                : [`to ${name(tr.targetId)}`, tr.kind === 'file' ? sizeText(tr.size) : null, ago(tr.createdAt)]
              )
                .filter(Boolean)
                .join(' · ')}
              right={<Badge label={b.label} tone={b.tone} />}
              below={
                tr.progress ? (
                  <ProgressBar percent={percent(tr)} muted={tr.progress.phase === 'sealing'} />
                ) : null
              }
              onPress={() => router.push('/activity')}
            />
          );
        })}
      </Group>

      {k.info && (
        <Text style={[styles.footer, { color: t.muted }]}>kitedrop core {k.info.version}</Text>
      )}
    </ScrollView>
  );
}

function status(d: Device): string {
  if (d.platform === 'ios') return 'gets items when its app opens';
  if (d.online) return 'online';
  return d.lastSeen ? `last seen ${ago(d.lastSeen)}` : 'offline';
}

const styles = StyleSheet.create({
  page: { padding: 16, paddingBottom: 40 },
  topbar: { flexDirection: 'row', alignItems: 'center', gap: 12, marginBottom: 6 },
  brand: { fontSize: 24, fontWeight: '700', letterSpacing: -0.3 },
  battery: { flexDirection: 'row', gap: 12, alignItems: 'flex-start' },
  deviceHead: { flexDirection: 'row', alignItems: 'center', gap: 12 },
  name: { fontSize: 17, fontWeight: '600' },
  actions: { flexDirection: 'row', gap: 8, marginTop: 12 },
  add: {
    flexDirection: 'row',
    gap: 8,
    alignItems: 'center',
    justifyContent: 'center',
    borderWidth: 1.5,
    borderStyle: 'dashed',
    borderRadius: 16,
    paddingVertical: 14,
  },
  hint: { textAlign: 'center', marginTop: 10, lineHeight: 20, paddingHorizontal: 12 },
  footer: { marginTop: 28, textAlign: 'center', fontSize: 12 },
});
