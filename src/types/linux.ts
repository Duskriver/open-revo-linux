import type { FanCurvePoint } from './controls';
export interface ReadingMetadata {
  source: string;
  /** Last successful sample; absent for unavailable, faulted or warming-up readings. */
  sampled_at_ms: number | null;
  status: 'ok' | 'unavailable' | 'fault' | 'warming_up';
}

export interface LinuxSensor extends ReadingMetadata {
  id: string;
  device: string;
  label: string;
  kind: 'temperature' | 'fan' | 'power';
  value: number | null;
  unit: string;
}

export interface LinuxBattery {
  id: string;
  capacity: number | null;
  status: string | null;
  health_percent: number | null;
  charge_limit: number | null;
  charge_limit_writable: boolean;
  power_w: number | null;
}

export interface LinuxBacklight {
  id: string;
  kind: 'backlight' | 'leds';
  percent: number | null;
}

export interface LinuxSnapshot {
  platform: 'linux';
  domains: LinuxDomains;
  control_error: string | null;
  hotkeys_enabled: boolean;
  hotkey_error: string | null;
  is_mock: false;
  timestamp_ms: number;
  telemetry: Record<string, ReadingMetadata>;
  model: string | null;
  vendor: string | null;
  board: string | null;
  bios: string | null;
  kernel: string | null;
  cpu: string | null;
  cpu_usage: number | null;
  cpu_frequency_mhz: number | null;
  memory_used_gib: number | null;
  memory_total_gib: number | null;
  ac_online: boolean | null;
  sensors: LinuxSensor[];
  batteries: LinuxBattery[];
  backlights: LinuxBacklight[];
  power: {
    active: string | null;
    available: string[];
    source: string | null;
    can_set: boolean | null;
    error: string | null;
  };
  capabilities: { id: string; name: string; available: boolean; detail: string }[];
  oem: LinuxOemSnapshot;
  wmi_events: { available: boolean; last_sequence: number | null; last_code: number | null; dropped: number; error: string | null };
  controls: {
    platform: { active: string | null; available: string[]; source: string | null; writable: boolean };
    fans: LinuxFanControl[];
    power_limits: LinuxPowerLimit[];
    fan_error: string | null;
  };
}

export interface LinuxFanControl {
  id: string;
  name: string;
  rpm: number | null;
  mode: number | null;
  boosted: boolean;
  writable: boolean;
  reason: string | null;
}

export interface LinuxPowerLimit {
  id: string;
  name: string;
  watts: number;
  min_watts: number;
  max_watts: number;
  writable: boolean;
  reason: string | null;
}

export interface CpuLimits { pl1: number; pl2: number; pl4: number }
export interface LinuxOemSnapshot {
  control_abi?: number | null;
  control_backend?: string;
  control_fault?: boolean;
  water_cooling_supported?: boolean | null;
  available: boolean;
  writable: boolean;
  error: string | null;
  mode: number | null;
  custom_power: boolean;
  fan_boost: boolean;
  fan_rpm: number[];
  dc_jack: boolean;
  presets: CpuLimits[];
  limits: CpuLimits | null;
  fan_curve: { supported: boolean; active: boolean; fault: boolean; applied: LinuxFanCurveConfig | null };
}

export interface LinuxFanCurveConfig {
  cpu: FanCurvePoint[];
  gpu: FanCurvePoint[];
  isolated: boolean;
  ramp_enabled: boolean;
  ramp_ms: number;
}

export type LinuxPowerMode = 'office' | 'balanced' | 'turbo' | 'custom';
export interface LinuxDomains {
  devices: {
    battery: { supported: boolean; writable: boolean; mode: 'full' | 'balanced' | 'longevity' | 'custom' | null; lower: number | null; upper: number | null; error: string | null };
    switches: { id: DeviceSwitch; enabled: boolean | null; writable: boolean; detail: string }[];
    fault: boolean; error: string | null;
  };
  performance: {
    active: LinuxPowerMode | null;
    modes: { mode: LinuxPowerMode; enabled: boolean; reason: string | null }[];
    limits: CpuLimits | null;
    ceiling: CpuLimits | null;
    limits_writable: boolean;
    power_limits: LinuxPowerLimit[];
    error: string | null;
  };
  cooling: {
    rpm: (number | null)[];
    boosted: boolean;
    writable: boolean;
    error: string | null;
    curve: LinuxOemSnapshot['fan_curve'];
    curve_writable: boolean;
  };
}
export interface LinuxAppConfig {
  version: 3;
  hotkeys_enabled?: boolean;
  fan_curve_draft: LinuxFanCurveConfig | null;
  mode: LinuxPowerMode | null;
  custom_limits: CpuLimits | null;
  legacy_scenes?: unknown[];
}

export type DeviceSwitch = "win_lock" | "fn_lock" | "camera" | "usb_charge" | "ac_recovery" | "touchpad";
