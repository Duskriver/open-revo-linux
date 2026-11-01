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

export interface LinuxAppConfig {
  version: 1;
  fan_curve_draft: LinuxFanCurveConfig | null;
}
