import { invoke, isTauri } from '@tauri-apps/api/core';
import type { CpuLimits, LinuxAppConfig, LinuxFanCurveConfig, LinuxSnapshot, LinuxPowerMode } from '../types/linux';

export interface LinuxMutationArgs {
  mode: { mode: LinuxPowerMode };
  fan: { enabled: boolean };
  cpu_limits: { limits: CpuLimits };
  fan_curve: { config: LinuxFanCurveConfig };
  restore_fan_curve: Record<string, never>;
  battery: { policy: { mode: 'full' | 'balanced' | 'longevity' | 'custom'; lower: number | null; upper: number | null } };
  switch: { switch: import('../types/linux').DeviceSwitch; enabled: boolean };
  hotkeys: { enabled: boolean };
  battery_limit: { battery: string; limit: number };
  backlight: { device: string; percent: number };
  power_limit: { id: string; watts: number };
  set_autostart_enabled: { enabled: boolean };
}
export type LinuxMutation = keyof LinuxMutationArgs;
export type LinuxAction = <C extends LinuxMutation>(command: C, args: LinuxMutationArgs[C]) => Promise<boolean>;
export function errorMessage(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'message' in error && typeof error.message === 'string') return error.message;
  return String(error);
}
let configQueue: Promise<unknown> = Promise.resolve();
function configRequest<T>(operation: () => Promise<T>): Promise<T> {
  const result = configQueue.then(operation);
  configQueue = result.catch(() => undefined);
  return result;
}
export const linuxApi = {
  isNative: isTauri,
  getStatus: () => invoke<LinuxSnapshot>('get_linux_status'),
  getAutostart: () => invoke<boolean>('get_autostart_enabled'),
  loadConfig: () => configRequest(() => invoke<LinuxAppConfig | null>('get_linux_config')),
  saveFanCurveDraft: (draft: LinuxFanCurveConfig) => configRequest(() => invoke<LinuxAppConfig>('save_linux_fan_curve_draft', { draft })),
  exportConfig: () => configRequest(() => invoke<string | null>('export_linux_config')),
  importConfig: (json: string) => configRequest(() => invoke<LinuxAppConfig>('import_linux_config', { json })),
  mutate: <C extends LinuxMutation>(command: C, args: LinuxMutationArgs[C]): Promise<void> => command === 'set_autostart_enabled'
    ? invoke(command, { ...args }) : invoke('execute_linux_action', { action: { kind: command, ...args } }),
};
