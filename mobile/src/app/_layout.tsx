import { Stack } from 'expo-router';
import { StatusBar } from 'expo-status-bar';

import { useTheme } from '@/lib/theme';

export default function RootLayout() {
  const t = useTheme();
  return (
    <>
      <StatusBar style="auto" />
      <Stack
        screenOptions={{
          headerStyle: { backgroundColor: t.bg },
          headerShadowVisible: false,
          headerTintColor: t.accent,
          headerTitleStyle: { color: t.text },
          contentStyle: { backgroundColor: t.bg },
        }}
      >
        <Stack.Screen name="index" options={{ title: 'kitedrop', headerShown: false }} />
        <Stack.Screen name="activity" options={{ title: 'Activity' }} />
        <Stack.Screen name="add" options={{ title: 'Add a device' }} />
        <Stack.Screen name="pair" options={{ title: 'Pair' }} />
        <Stack.Screen
          name="compose"
          options={{ title: 'Send text', presentation: 'modal', headerStyle: { backgroundColor: t.card } }}
        />
        <Stack.Screen name="device" options={{ title: '' }} />
      </Stack>
    </>
  );
}
