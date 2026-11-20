import { router, useLocalSearchParams } from 'expo-router';
import { useState } from 'react';
import { Alert, ScrollView, StyleSheet, Text, View } from 'react-native';

import { Avatar, Button, Card, Field, Heading, Notice } from '@/components/ui';
import { Kitedrop, PLATFORM_NAME, ago } from '@/lib/kitedrop';
import { useTheme } from '@/lib/theme';

export default function DeviceScreen() {
  const t = useTheme();
  const { id } = useLocalSearchParams<{ id: string }>();
  const device = Kitedrop.devices().find((d) => d.id === id);
  const [name, setName] = useState(device?.name ?? '');
  const [error, setError] = useState<string | null>(null);

  if (!device) {
    return <Text style={{ color: t.muted, padding: 16 }}>This device is no longer in your group.</Text>;
  }

  async function rename() {
    try {
      await Kitedrop.renameDevice(device!.id, name.trim());
      router.back();
    } catch (e) {
      setError(String(e));
    }
  }

  function remove() {
    Alert.alert(`Remove ${device!.name}?`, 'It will no longer be able to send to or receive from your devices.', [
      { text: 'Cancel', style: 'cancel' },
      {
        text: 'Remove',
        style: 'destructive',
        onPress: async () => {
          try {
            await Kitedrop.removeDevice(device!.id);
            router.back();
          } catch (e) {
            setError(String(e));
          }
        },
      },
    ]);
  }

  const status =
    device.platform === 'ios'
      ? 'Gets items when its app opens'
      : device.online
        ? 'Online'
        : device.lastSeen
          ? `Last seen ${ago(device.lastSeen)}`
          : 'Offline';

  return (
    <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.page}>
      <View style={styles.head}>
        <Avatar platform={device.platform} online={device.platform === 'ios' ? null : device.online} size={72} />
        <Text style={[styles.title, { color: t.text }]}>{device.name}</Text>
        <Text style={{ color: t.muted }}>
          {PLATFORM_NAME[device.platform]} · {status}
        </Text>
        {device.address && <Text style={{ color: t.muted, fontSize: 12, marginTop: 2 }}>{device.address}</Text>}
      </View>

      <Heading>Name</Heading>
      <Card>
        <Field value={name} onChangeText={setName} autoCorrect={false} returnKeyType="done" onSubmitEditing={rename} />
        <Button title="Save name" primary onPress={rename} disabled={!name.trim() || name.trim() === device.name} />
      </Card>

      <Heading>Remove</Heading>
      <Card>
        <Text style={{ color: t.muted, marginBottom: 12, lineHeight: 20 }}>
          Removing it takes it out of your group on every device. To use it again, pair it again.
        </Text>
        <Button title="Remove from my devices" variant="danger" icon="trash" onPress={remove} />
      </Card>
      {error && <Notice ok={false} text={error} />}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  page: { padding: 16, paddingBottom: 40 },
  head: { alignItems: 'center', gap: 4, marginTop: 4 },
  title: { fontSize: 22, fontWeight: '700', marginTop: 10 },
});
