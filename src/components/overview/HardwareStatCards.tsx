import { Activity, Cpu, Fan, Sparkles } from 'lucide-react';
import { ReactNode } from 'react';
import { HardwareState } from '../../types/hardware';
import { ModelCapabilities } from '../../plugins/plugin_manifest';

export interface HardwareTelemetry {
  cpuName?: string | null;
  cpuTemp?: number | null;
  cpuUsage?: number | null;
  cpuFrequency?: number | null;
  cpuPower?: number | null;
  gpuName?: string | null;
  gpuTemp?: number | null;
  gpuUsage?: number | null;
  gpuFrequency?: number | null;
  gpuPower?: number | null;
  fanRpm?: (number | null)[];
  fanBoost?: boolean;
  fanState?: string;
  memoryLabel?: string;
  memoryUsed?: number | null;
  memoryTotal?: number | null;
  memoryFrequency?: number | null;
}

const format = (n: number | null | undefined, digits = 0) =>
  n == null || !Number.isFinite(n) ? '—' : n.toFixed(digits);

function StatCard({ kind, title, icon, children, footer, usage }: {
  kind: string; title: string; icon: ReactNode; children: ReactNode; footer: ReactNode; usage?: number | null;
}) {
  return <div className={`glass-card telemetry-card telemetry-card-${kind} shared-stat-card`} data-slot={kind}>
    <div className="shared-stat-header"><span>{title}</span>{icon}</div>
    <div className="shared-stat-readings">{children}</div>
    <progress className="shared-stat-progress" max={100} value={usage == null ? 0 : Math.max(0, Math.min(100, usage))}
      aria-label={`${title}负载`} aria-valuetext={usage == null ? '未提供' : `${usage.toFixed(0)}%`} />
    <div className="shared-stat-footer">{footer}</div>
  </div>;
}

/** Both platforms supply measurements; unavailable values never become demo readings. */
export function HardwareStatCards({ status, capabilities, telemetry }: {
  status?: HardwareState; capabilities?: ModelCapabilities; telemetry?: HardwareTelemetry;
}) {
  const t: HardwareTelemetry = telemetry ?? {
    cpuName: status?.cpu_name, cpuTemp: status?.cpu_temp, cpuUsage: status?.cpu_usage,
    cpuFrequency: status?.cpu_freq, cpuPower: status?.cpu_power_w,
    gpuName: status?.gpu_name, gpuTemp: status?.gpu_temp, gpuUsage: status?.gpu_usage,
    gpuFrequency: status?.gpu_core_freq, gpuPower: status?.gpu_power_w,
    fanRpm: [status?.cpu_fan_rpm ?? null, status?.gpu_fan_rpm ?? null], fanBoost: status?.fan_boost,
    fanState: status?.fan_boost ? '一键强冷' : '自动温控',
    memoryLabel: capabilities?.discrete_gpu === false ? 'UMA 共享显存' : 'VRAM 显存容量',
    memoryUsed: status?.gpu_vram_used_mb == null ? null : status.gpu_vram_used_mb / 1024,
    memoryTotal: status?.gpu_vram_mb == null ? null : status.gpu_vram_mb / 1024,
    memoryFrequency: status?.gpu_mem_freq,
  };
  const memoryUsage = t.memoryUsed != null && t.memoryTotal ? t.memoryUsed / t.memoryTotal * 100 : null;
  return <div className="telemetry-cards-grid" data-slot="telemetry-grid">
    <StatCard kind="cpu" title="CPU 处理器状态" icon={<Cpu size={16} />} usage={t.cpuUsage}
      footer={<><span title={t.cpuName ?? ''}>{t.cpuName ?? 'CPU 信息未提供'}</span><b>{format(t.cpuUsage)}%</b></>}>
      <div><strong className={(t.cpuTemp ?? 0) > 85 ? 'shared-stat-hot' : ''}>{format(t.cpuTemp)}</strong><small>°C</small></div>
      <div><b>{format(t.cpuFrequency)}</b><small>MHz</small></div>
      <span className="shared-stat-power">{format(t.cpuPower, 1)} W</span>
    </StatCard>
    <StatCard kind="gpu" title="GPU 显卡状态" icon={<Activity size={16} />} usage={t.gpuUsage}
      footer={<><span title={t.gpuName ?? ''}>{t.gpuName ?? 'GPU 信息未提供'}</span><b>{format(t.gpuUsage)}%</b></>}>
      <div><strong className={(t.gpuTemp ?? 0) > 80 ? 'shared-stat-hot' : ''}>{format(t.gpuTemp)}</strong><small>°C</small></div>
      <div><b>{format(t.gpuFrequency)}</b><small>MHz</small></div>
      <span className="shared-stat-power">{format(t.gpuPower, 1)} W</span>
    </StatCard>
    <StatCard kind="fan" title="散热系统状态" icon={<Fan size={16} className={t.fanBoost ? 'animate-spin' : ''} />}
      footer={<><span>风扇转速</span><b>{t.fanState ?? '状态未提供'}</b></>}>
      {(t.fanRpm?.length ? t.fanRpm : [null, null]).map((rpm, i) =>
        <div key={i}><small>风扇 {i + 1}</small><div><strong>{format(rpm)}</strong><small>RPM</small></div></div>)}
    </StatCard>
    <StatCard kind="vram" title={t.memoryLabel ?? '内存容量'} icon={<Sparkles size={16} />} usage={memoryUsage}
      footer={<><span>{t.memoryFrequency == null ? '已用 / 总容量' : `${format(t.memoryFrequency)} MHz`}</span><b>{format(memoryUsage)}%</b></>}>
      <div><strong>{format(t.memoryUsed, 1)}</strong><small>/ {format(t.memoryTotal, 1)} GiB</small></div>
    </StatCard>
  </div>;
}
