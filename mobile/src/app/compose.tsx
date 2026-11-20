import * as Clipboard from 'expo-clipboard';
import { router, useLocalSearchParams } from 'expo-router';
import { useState } from 'react';
import { KeyboardAvoidingView, Platform, StyleSheet, Text, View } from 'react-native';

import { Avatar, Button, Field, Notice } from '@/components/ui';
import { Kitedrop, looksLikeLink } from '@/lib/kitedrop';
import { useTheme } from '@/lib/theme';

export default function Compose() {
  const t = useTheme();
  const { to } = useLocalSearchParams<{ to: string }>();
  const [text, setText] = useState('');
  const [error, setError] = useState<string | null>(null);
  const target = Kitedrop.devices().find((d) => d.id === to);
  const isLink = looksLikeLink(text);

  async function send() {
    try {
      if (isLink) await Kitedrop.sendLink([to], text.trim());
      else await Kitedrop.sendText([to], text);
      router.back();
    } catch (e) {
      setError(String(e));
    }
  }

  return (
    <KeyboardAvoidingView
      behavior={Platform.OS === 'ios' ? 'padding' : undefined}
      style={[styles.page, { backgroundColor: t.card }]}
    >
      <View style={styles.to}>
        <Text style={{ color: t.muted }}>To</Text>
        {target && <Avatar platform={target.platform} size={26} />}
        <Text style={{ color: t.text, fontWeight: '600' }}>{target?.name ?? 'device'}</Text>
      </View>
      <Field
        value={text}
        onChangeText={setText}
        multiline
        autoFocus
        placeholder="Text or link"
        style={styles.input}
      />
      <View style={styles.row}>
        <Button title="Paste" icon="clipboard" onPress={async () => setText(await Clipboard.getStringAsync())} />
        <Button
          title={isLink ? 'Send link' : 'Send'}
          icon="send"
          primary
          flex
          onPress={send}
          disabled={!text}
        />
      </View>
      {error && <Notice ok={false} text={error} />}
    </KeyboardAvoidingView>
  );
}

const styles = StyleSheet.create({
  page: { flex: 1, padding: 16 },
  to: { flexDirection: 'row', alignItems: 'center', gap: 8, marginBottom: 12 },
  input: { minHeight: 160, textAlignVertical: 'top' },
  row: { flexDirection: 'row', gap: 8 },
});
