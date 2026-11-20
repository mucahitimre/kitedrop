import { useState } from 'react';
import { ScrollView, StyleSheet, Text, View } from 'react-native';

import { Badge, Button, Group, KindIcon, ProgressBar, Row, Segmented } from '@/components/ui';
import {
  Kitedrop,
  ago,
  itemTitle,
  percent,
  progressText,
  reasonText,
  sizeText,
  stateBadge,
  useKitedrop,
  type Received,
  type Transfer,
} from '@/lib/kitedrop';
import { useTheme } from '@/lib/theme';

export default function Activity() {
  const k = useKitedrop();
  const [tab, setTab] = useState<'sent' | 'received'>('sent');
  const name = (id: string) => k.devices.find((d) => d.id === id)?.name ?? 'removed device';

  return (
    <ScrollView contentInsetAdjustmentBehavior="automatic" contentContainerStyle={styles.page}>
      <Segmented
        value={tab}
        onChange={setTab}
        options={[
          { value: 'sent', label: `Sent (${k.transfers.length})` },
          { value: 'received', label: `Received (${k.received.length})` },
        ]}
      />
      {tab === 'sent' && k.transfers.length === 0 && <Empty text="Nothing sent yet." />}
      {tab === 'received' && k.received.length === 0 && (
        <Empty text="Nothing received yet. Text and links go straight to the clipboard; photos to Photos, other files to Files." />
      )}
      <Group>
        {tab === 'sent'
          ? k.transfers.map((tr) => <SentRow key={tr.id} tr={tr} to={name(tr.targetId)} />)
          : k.received.map((r) => <ReceivedRow key={r.id} r={r} from={name(r.senderId)} />)}
      </Group>
    </ScrollView>
  );
}

function Empty({ text }: { text: string }) {
  const t = useTheme();
  return <Text style={[styles.empty, { color: t.muted }]}>{text}</Text>;
}

function SentRow({ tr, to }: { tr: Transfer; to: string }) {
  const b = stateBadge(tr);
  const open = tr.state === 'waiting' || tr.state === 'queued' || tr.state === 'sending';
  const reason = tr.reason && tr.reason !== 'cancelled' ? reasonText(tr.reason) : null;
  return (
    <View>
      <Row
        left={<KindIcon kind={tr.kind} />}
        title={itemTitle(tr)}
        sub={(progressText(tr)
          ? [`to ${to}`, progressText(tr)]
          : [`to ${to}`, tr.kind === 'file' ? sizeText(tr.size) : null, ago(tr.createdAt), reason]
        )
          .filter(Boolean)
          .join(' · ')}
        right={<Badge label={b.label} tone={b.tone} />}
        below={
          tr.progress ? <ProgressBar percent={percent(tr)} muted={tr.progress.phase === 'sealing'} /> : null
        }
      />
      {open && (
        <View style={styles.actions}>
          {tr.state === 'waiting' && !tr.viaOutbox && (
            <Button title="Retry now" flex onPress={() => Kitedrop.retry(tr.id)} />
          )}
          <Button title="Cancel" variant="danger" flex onPress={() => Kitedrop.cancel(tr.id)} />
        </View>
      )}
    </View>
  );
}

function ReceivedRow({ r, from }: { r: Received; from: string }) {
  const inPhotos = r.location?.startsWith('photos:') ?? false;
  const where = r.kind !== 'file' ? 'copied' : inPhotos ? 'in Photos' : null;
  return (
    <Row
      left={<KindIcon kind={inPhotos ? 'photo' : r.kind} />}
      title={itemTitle(r)}
      sub={[`from ${from}`, r.kind === 'file' ? sizeText(r.size) : null, ago(r.at), where].filter(Boolean).join(' · ')}
    />
  );
}

const styles = StyleSheet.create({
  page: { padding: 16, paddingBottom: 40 },
  empty: { textAlign: 'center', marginTop: 24, lineHeight: 20, paddingHorizontal: 16 },
  actions: { flexDirection: 'row', gap: 8, paddingHorizontal: 14, paddingBottom: 12, marginLeft: 44 },
});
