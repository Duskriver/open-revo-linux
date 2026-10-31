export interface LinuxSensor {
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
}
