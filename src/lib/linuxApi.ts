import { invoke, isTauri } from '@tauri-apps/api/core';
import type { CpuLimits, LinuxAppConfig, LinuxFanCurveConfig, LinuxSnapshot } from '../types/linux';

/** Arguments mirror the Linux host's public IPC contract. */
export interface LinuxMutationArgs {
  set_linux_power_profile: { profile: string };
  set_linux_platform_profile: { profile: string };
  set_linux_fan_boost: { id: string; enabled: boolean };
  set_linux_power_limit: { id: string; watts: number };
  set_linux_oem_mode: { mode: number };
  set_linux_oem_fan_boost: { enabled: boolean };
  set_linux_oem_cpu_limits: CpuLimits;
  set_linux_oem_fan_curve: { config: LinuxFanCurveConfig };
  restore_linux_oem_fan_curve: Record<string, never>;
  set_linux_battery_limit: { battery: string; limit: number };
  set_linux_backlight: { device: string; percent: number };
  set_autostart_enabled: { enabled: boolean };
}

export type LinuxMutation = keyof LinuxMutationArgs;
export type LinuxAction = <C extends LinuxMutation>(command: C, args: LinuxMutationArgs[C]) => Promise<boolean>;

export function errorMessage(error: unknown): string {
  if (typeof error === 'object' && error !== null && 'message' in error && typeof error.message === 'string') {
    return error.message;
  }
  return String(error);
}

// Keep a remounted editor behind any save/import still running in another view.
let configQueue: Promise<void> = Promise.resolve();
function configRequest<T>(operation: () => Promise<T>): Promise<T> {
  const result = configQueue.then(operation);
  configQueue = result.then(() => undefined, () => undefined);
  return result;
}

/** The only transport used by Linux views; browser previews never simulate hardware. */
export const linuxApi = {
  isNative: isTauri,
  getStatus: () => invoke<LinuxSnapshot>('get_linux_status'),
  renewControlLease: () => invoke<void>('renew_linux_control_lease'),
  getAutostart: () => invoke<boolean>('get_autostart_enabled'),
  loadConfig: () => configRequest(() => invoke<LinuxAppConfig | null>('get_linux_config')),
  saveFanCurveDraft: (draft: LinuxFanCurveConfig) => configRequest(() => invoke<LinuxAppConfig>('save_linux_fan_curve_draft', { draft })),
  exportConfig: () => configRequest(() => invoke<string | null>('export_linux_config')),
  importConfig: (json: string) => configRequest(() => invoke<LinuxAppConfig>('import_linux_config', { json })),
  mutate: <C extends LinuxMutation>(command: C, args: LinuxMutationArgs[C]) => invoke<void>(command, { ...args }),
};
