import { router, useLocalSearchParams } from 'expo-router';
import { useState } from 'react';
import { ActivityIndicator, StyleSheet, Text, View } from 'react-native';

import { Button, Card, Icon, Notice } from '@/components/ui';
import { Kitedrop } from '@/lib/kitedrop';
import { useTheme } from '@/lib/theme';

/**
 * Deep link `kitedrop://pair?…`: the phone's own camera app scanned a kitedrop pairing code.
 * Pairing still asks for a tap, so a link opened by accident does nothing.
 */
export default function PairLink() {
  const t = useTheme();
  const params = useLocalSearchParams<Record<string, string>>();
  const [state, setState] = useState<'ready' | 'busy' | 'done' | 'error'>('ready');
  const [text, setText] = useState('');

  const query = ['v', 'c', 'f', 'a', 'r']
    .filter((k) => typeof params[k] === 'string')
    .map((k) => `${k}=${params[k]}`)
    .join('&');
  const uri = `kitedrop://pair?${query}`;

  async function go() {
    setState('busy');
    try {
      const id = await Kitedrop.pair(uri);
      const name = Kitedrop.devices().find((d) => d.id === id)?.name ?? 'the device';
      setText(`Paired with ${name}.`);
      setState('done');
      setTimeout(() => router.replace('/'), 1200);
    } catch (e) {
      const s = String(e);
      setText(
        s.includes('code_invalid')
          ? 'The code has expired or was already used.'
          : s.includes('already_in_group')
            ? 'Both devices are already in groups with other devices. Remove one of them from its group first.'
            : s,
      );
      setState('error');
    }
  }

  return (
    <View style={styles.page}>
      <Card style={styles.card}>
        <View style={[styles.badge, { backgroundColor: t.accentSoft }]}>
          <Icon name="pair" size={30} color={t.accent} />
        </View>
        <Text style={[styles.lead, { color: t.text }]}>
          Add the device that showed this code to your kitedrop devices?
        </Text>
        {state === 'busy' ? (
          <ActivityIndicator />
        ) : (
          <View style={{ alignSelf: 'stretch' }}>
            <Button title="Pair" primary onPress={go} disabled={state === 'done'} />
          </View>
        )}
      </Card>
      {text !== '' && <Notice ok={state === 'done'} text={text} />}
    </View>
  );
}

const styles = StyleSheet.create({
  page: { padding: 16 },
  card: { alignItems: 'center', gap: 14, paddingVertical: 24 },
  badge: { width: 64, height: 64, borderRadius: 20, alignItems: 'center', justifyContent: 'center' },
  lead: { fontSize: 17, textAlign: 'center', lineHeight: 23, fontWeight: '500' },
});
