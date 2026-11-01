import { invoke, isTauri } from '@tauri-apps/api/core';
import type { CpuLimits, LinuxFanCurveConfig, LinuxSnapshot } from '../types/linux';

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

/** The only transport used by Linux views; browser previews never simulate hardware. */
export const linuxApi = {
  isNative: isTauri,
  getStatus: () => invoke<LinuxSnapshot>('get_linux_status'),
  getAutostart: () => invoke<boolean>('get_autostart_enabled'),
  mutate: <C extends LinuxMutation>(command: C, args: LinuxMutationArgs[C]) => invoke<void>(command, { ...args }),
};
