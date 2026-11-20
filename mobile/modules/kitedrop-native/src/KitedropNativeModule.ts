import { NativeModule, requireNativeModule } from 'expo';

export type Platform = 'ios' | 'android' | 'macos' | 'windows';

export type Info = {
  deviceId: string;
  name: string;
  platform: Platform;
  version: string;
  /** Android: listening port. */
  port: number | null;
};

export type Device = {
  id: string;
  name: string;
  platform: Platform;
  isSelf: boolean;
  online: boolean;
  address: string | null;
  lastSeen: number | null;
};

export type TransferState = 'queued' | 'sending' | 'delivered' | 'waiting' | 'failed';

export type Transfer = {
  id: string;
  targetId: string;
  kind: 'file' | 'text' | 'link';
  name: string;
  size: number;
  state: TransferState;
  reason: string | null;
  viaOutbox: boolean;
  createdAt: number;
  updatedAt: number;
  /** Large sends being encrypted (`sealing`) or uploaded right now. */
  progress: { phase: 'sealing' | 'uploading'; done: number; total: number } | null;
};

export type Received = {
  id: string;
  senderId: string;
  kind: 'file' | 'text' | 'link';
  name: string;
  size: number;
  location: string | null;
  at: number;
};

export type Invite = { uri: string; size: number; modules: number[] };

type Events = { onChange(event: { type: string }): void };

declare class KitedropNativeModule extends NativeModule<Events> {
  start(): Info;
  devices(): Device[];
  transfers(limit: number): Transfer[];
  received(limit: number): Received[];
  diagnostics(): string[];
  refresh(): void;
  retry(id: string): void;
  cancel(id: string): void;
  /** Android only (iPhones do not listen). */
  createInvite(reverse: boolean): Invite;
  pair(uri: string): Promise<string>;
  sendText(targets: string[], text: string): Promise<string[]>;
  sendLink(targets: string[], url: string): Promise<string[]>;
  sendFile(targets: string[], path: string, name: string | null, mime: string | null): Promise<string[]>;
  removeDevice(id: string): Promise<void>;
  renameDevice(id: string, name: string): Promise<void>;
  // Android
  isIgnoringBatteryOptimizations(): boolean;
  requestIgnoreBatteryOptimizations(): void;
  // iOS
  requestPhotosAccess(): Promise<boolean>;
  pollOutboxes(): Promise<number>;
  setPushToken(token: string | null): void;
}

export default requireNativeModule<KitedropNativeModule>('KitedropNative');
