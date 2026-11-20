import { CameraView, useCameraPermissions } from 'expo-camera';
import * as Clipboard from 'expo-clipboard';
import { router } from 'expo-router';
import { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Platform, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Button, Card, Field, Icon, Notice, Qr, Segmented } from '@/components/ui';
import { Kitedrop } from '@/lib/kitedrop';
import type { Invite } from '../../modules/kitedrop-native/src/KitedropNativeModule';
import { useTheme } from '@/lib/theme';

type Mode = 'scan' | 'show' | 'paste';

export default function Add() {
  const [mode, setMode] = useState<Mode>('scan');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  const used = useRef(false);

  async function pair(uri: string) {
    if (used.current) return;
    if (!uri.trim().startsWith('kitedrop://pair?')) {
      setMessage({ ok: false, text: 'This is not a kitedrop pairing code.' });
      return;
    }
    used.current = true;
    setBusy(true);
    setMessage(null);
    try {
      const id = await Kitedrop.pair(uri.trim());
      const name = Kitedrop.devices().find((d) => d.id === id)?.name ?? 'the device';
      setMessage({ ok: true, text: `Paired with ${name}.` });
      setTimeout(() => router.back(), 1200);
    } catch (e) {
      used.current = false;
      const s = String(e);
      setMessage({
        ok: false,
        text: s.includes('code_invalid')
          ? 'The code has expired or was already used. Show a new one.'
          : s.includes('already_in_group')
            ? 'Both devices are already in groups with other devices. Remove one of them from its group first.'
            : s.includes('network')
              ? 'The other device could not be reached. Are both on the same Wi-Fi?'
              : s,
      });
    } finally {
      setBusy(false);
    }
  }

  const options: { value: Mode; label: string; icon: 'scan' | 'qr' | 'clipboard' }[] = [
    { value: 'scan', label: 'Scan', icon: 'scan' },
    ...(Platform.OS === 'android' ? [{ value: 'show' as const, label: 'Show code', icon: 'qr' as const }] : []),
    { value: 'paste', label: 'Paste link', icon: 'clipboard' },
  ];

  return (
    <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.page}>
      <Segmented
        value={mode}
        onChange={(m) => {
          setMode(m);
          setMessage(null);
        }}
        options={options}
      />
      {mode === 'scan' && <Scan onCode={pair} />}
      {mode === 'show' && <Show />}
      {mode === 'paste' && <Paste onLink={pair} />}
      {busy && <ActivityIndicator style={{ marginTop: 16 }} />}
      {message && <Notice ok={message.ok} text={message.text} />}
    </ScrollView>
  );
}

function Step({ children }: { children: string }) {
  const t = useTheme();
  return <Text style={[styles.step, { color: t.muted }]}>{children}</Text>;
}

function Scan({ onCode }: { onCode: (uri: string) => void }) {
  const t = useTheme();
  const [permission, requestPermission] = useCameraPermissions();
  if (!permission) return <View />;
  if (!permission.granted) {
    return (
      <Card style={styles.center}>
        <Icon name="scan" size={36} color={t.accent} />
        <Text style={[styles.lead, { color: t.text }]}>
          kitedrop needs the camera to scan the code shown on your other device.
        </Text>
        <Button title="Allow camera" primary onPress={requestPermission} />
      </Card>
    );
  }
  return (
    <View>
      <Step>On your computer open kitedrop → Add device, then point the camera at the code.</Step>
      <View style={styles.cameraBox}>
        <CameraView
          style={StyleSheet.absoluteFill}
          barcodeScannerSettings={{ barcodeTypes: ['qr'] }}
          onBarcodeScanned={(r) => onCode(r.data)}
        />
        <View pointerEvents="none" style={styles.frame}>
          {(['tl', 'tr', 'bl', 'br'] as const).map((c) => (
            <View key={c} style={[styles.corner, CORNER[c]]} />
          ))}
        </View>
      </View>
    </View>
  );
}

function createInvite(): { invite: Invite | null; error: string | null } {
  try {
    return { invite: Kitedrop.createInvite(false), error: null };
  } catch (e) {
    return { invite: null, error: String(e) };
  }
}

/** Android listens, so it can show a code that another device scans. */
function Show() {
  const t = useTheme();
  const [{ invite, error }, setMade] = useState(createInvite);
  const [left, setLeft] = useState(60);
  const [copied, setCopied] = useState(false);

  function make() {
    setMade(createInvite());
    setLeft(60);
    setCopied(false);
  }

  useEffect(() => {
    const id = setInterval(() => setLeft((s) => Math.max(0, s - 1)), 1000);
    return () => clearInterval(id);
  }, []);

  if (error) return <Notice ok={false} text={error} />;
  if (!invite) return null;
  return (
    <View>
      <Step>Scan this code with kitedrop on your other device.</Step>
      <Card style={styles.center}>
        {left > 0 ? (
          <Qr size={invite.size} modules={invite.modules} />
        ) : (
          <View style={[styles.expired, { backgroundColor: t.bg }]}>
            <Text style={{ color: t.muted }}>Code expired</Text>
          </View>
        )}
        <Text style={{ color: t.muted, marginTop: 4 }}>{left > 0 ? `Valid for ${left} s · one use` : 'Make a new one.'}</Text>
      </Card>
      <View style={styles.row}>
        <Button title="New code" flex onPress={make} primary={left === 0} />
        <Button
          title={copied ? 'Copied' : 'Copy link'}
          icon="link"
          flex
          disabled={left === 0}
          onPress={async () => {
            await Clipboard.setStringAsync(invite.uri);
            setCopied(true);
          }}
        />
      </View>
    </View>
  );
}

function Paste({ onLink }: { onLink: (uri: string) => void }) {
  const [text, setText] = useState('');
  return (
    <View>
      <Step>On the other device: Add device → Copy link. Paste it here within the minute.</Step>
      <Field
        value={text}
        onChangeText={setText}
        placeholder="kitedrop://pair?…"
        autoCapitalize="none"
        autoCorrect={false}
      />
      <View style={styles.row}>
        <Button title="Paste" icon="clipboard" flex onPress={async () => setText(await Clipboard.getStringAsync())} />
        <Button title="Pair" icon="pair" primary flex onPress={() => onLink(text)} disabled={!text} />
      </View>
    </View>
  );
}

const C = 28;
const CORNER = {
  tl: { top: 0, left: 0, borderTopWidth: 4, borderLeftWidth: 4, borderTopLeftRadius: 14 },
  tr: { top: 0, right: 0, borderTopWidth: 4, borderRightWidth: 4, borderTopRightRadius: 14 },
  bl: { bottom: 0, left: 0, borderBottomWidth: 4, borderLeftWidth: 4, borderBottomLeftRadius: 14 },
  br: { bottom: 0, right: 0, borderBottomWidth: 4, borderRightWidth: 4, borderBottomRightRadius: 14 },
};

const styles = StyleSheet.create({
  page: { padding: 16, paddingBottom: 40 },
  step: { marginBottom: 12, lineHeight: 20 },
  lead: { textAlign: 'center', lineHeight: 21, fontSize: 15 },
  center: { alignItems: 'center', gap: 12, paddingVertical: 20 },
  cameraBox: { width: '100%', aspectRatio: 1, borderRadius: 20, overflow: 'hidden', backgroundColor: '#000' },
  frame: { position: 'absolute', top: '18%', left: '18%', right: '18%', bottom: '18%' },
  corner: { position: 'absolute', width: C, height: C, borderColor: '#fff' },
  expired: { width: 240, height: 240, borderRadius: 12, alignItems: 'center', justifyContent: 'center' },
  row: { flexDirection: 'row', gap: 8 },
});
