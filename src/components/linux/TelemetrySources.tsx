import type { LinuxSnapshot, ReadingMetadata } from '../../types/linux';

const labels = {
  model: '机型', vendor: '厂商', board: '主板', bios: 'BIOS', kernel: '内核', cpu: '处理器',
  cpu_usage: 'CPU 使用率', cpu_frequency_mhz: 'CPU 频率', memory_used_gib: '已用内存',
  memory_total_gib: '总内存', ac_online: '外接供电',
};
const states: Record<ReadingMetadata['status'], string> = {
  ok: '已读取', unavailable: '未提供', fault: '读取失败', warming_up: '等待有效采样',
};

export function readingDescription(metadata: ReadingMetadata): string {
  const sampled = metadata.sampled_at_ms == null ? '无有效采样'
    : new Date(metadata.sampled_at_ms).toLocaleString('zh-CN', { hour12: false });
  return `${states[metadata.status] ?? '状态未知'} · ${metadata.source} · ${sampled}`;
}

export function TelemetrySources({ status }: { status: LinuxSnapshot | null }) {
  return <details className="sys-sub-panel linux-telemetry-sources">
    <summary>数据来源与采样状态</summary>
    <p className="sys-row-desc">机型等静态信息保留首次读取时间；动态数据每次重新采样。读取失败不会沿用旧值。</p>
    <div className="sys-rows">{Object.entries(status?.telemetry ?? {}).map(([key, metadata]) =>
      <div className="quick-row" key={key}><div>
        <div className="sys-row-label">{labels[key as keyof typeof labels] ?? key}</div>
        <div className="sys-row-desc">{readingDescription(metadata)}</div>
      </div></div>)}</div>
  </details>;
}
