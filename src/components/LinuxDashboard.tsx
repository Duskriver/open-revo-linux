import { EcBatteryControls, HardwareSwitches } from './linux/DeviceControls';
import { useEffect, useRef, useState } from 'react';
import { Activity, BatteryCharging, Download, Monitor, RefreshCw, Zap } from 'lucide-react';
import { RevCard } from './ui/RevCard';
import { DashboardChrome } from './common/DashboardChrome';
import { HardwareStatCards, HardwareTelemetry } from './overview/HardwareStatCards';
import { CpuPlSliderRow } from './tuning/CpuPlSliderRow';
import { ModuleErrorBoundary } from './ModuleErrorBoundary';
import { SponsorView } from './SponsorView';
import { useLinuxBackend } from '../hooks/useLinuxBackend';
import { linuxTabs } from './linux/navigation';
import { LinuxPowerLimit, LinuxSnapshot } from '../types/linux';
import { OemFanCurve } from './linux/OemFanCurve';
import { CoolingControls, PerformanceModes, CpuPowerControls } from './linux/DomainControls';
import { readingDescription, TelemetrySources } from './linux/TelemetrySources';

const batteryNames: Record<string, string> = {
  Charging: '充电中', Discharging: '使用电池', Full: '已充满', 'Not charging': '未充电', Unknown: '状态未知',
};
const value = (n: number | null | undefined, unit = '', digits = 0) =>
  n == null ? '未提供' : `${n.toFixed(digits)}${unit}`;

function BrightnessControl({ id, percent, disabled, onSet }: {
  id: string; percent: number | null; disabled: boolean; onSet: (percent: number) => void;
}) {
  const [draft, setDraft] = useState(percent ?? 50);
  const editing = useRef(false);
  useEffect(() => { if (!editing.current && percent != null) setDraft(percent); }, [percent]);
  const commit = () => { if (editing.current && !disabled) { editing.current = false; onSet(draft); } };
  return <label className="linux-brightness"><span>{id}</span>
    <input aria-label={`${id} 亮度`} type="range" min="5" max="100" value={draft} disabled={disabled}
      onChange={e => { editing.current = true; setDraft(Number(e.target.value)); }} onPointerUp={commit}
      onKeyUp={e => { if (['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End', 'PageUp', 'PageDown'].includes(e.key)) commit(); }}
      onBlur={commit} /><output>{draft}%</output>
  </label>;
}

function PowerLimitControl({ limit, disabled, onApply }: {
  limit: LinuxPowerLimit; disabled: boolean; onApply: (watts: number) => Promise<boolean>;
}) {
  const [draft, setDraft] = useState(limit.watts);
  const [dirty, setDirty] = useState(false);
  useEffect(() => { if (!dirty) setDraft(limit.watts); }, [limit.watts, dirty]);
  const valid = Number.isFinite(draft) && draft >= limit.min_watts && draft <= limit.max_watts;
  return <section className="sys-sub-panel linux-stack">
    <CpuPlSliderRow label={limit.name} value={draft} min={limit.min_watts} max={limit.max_watts} step={0.1}
      disabled={disabled || !limit.writable} onChange={n => { setDraft(n); setDirty(true); }} />
    <div className="linux-control-actions"><span className="linux-muted">当前读回 {value(limit.watts, ' W', 1)}</span>
      <button className="toggle-btn active-indigo" disabled={disabled || !limit.writable || !dirty || !valid}
        onClick={async () => { if (await onApply(draft)) setDirty(false); }}>应用功耗</button>
      {dirty && <button className="toggle-btn" disabled={disabled} onClick={() => { setDraft(limit.watts); setDirty(false); }}>还原输入</button>}
    </div>
    {!valid && <p className="linux-warning">请输入驱动允许范围内的功耗。</p>}
    {limit.reason && <p className="linux-muted">{limit.reason}</p>}
  </section>;
}

function telemetry(status: LinuxSnapshot | null): HardwareTelemetry {
  const sensor = (kind: string, device: RegExp) => status?.sensors.find(s => s.kind === kind && device.test(s.device))?.value;
  const gpu = status?.sensors.find(s => s.kind === 'temperature' && /amdgpu|nouveau|nvidia/i.test(s.device));
  return {
    cpuName: status?.cpu, cpuTemp: sensor('temperature', /k10temp|coretemp|zenpower/i),
    cpuUsage: status?.cpu_usage, cpuFrequency: status?.cpu_frequency_mhz,
    cpuPower: sensor('power', /k10temp|coretemp|zenpower|rapl/i),
    gpuName: gpu ? `${gpu.device} · ${gpu.label}` : null, gpuTemp: gpu?.value,
    gpuPower: sensor('power', /amdgpu|nouveau|nvidia/i),
    fanRpm: status?.domains.cooling.rpm, fanBoost: status?.domains.cooling.boosted,
    fanState: status?.domains.cooling.boosted ? '一键强冷' : '自动调速',
    memoryLabel: '系统内存', memoryUsed: status?.memory_used_gib, memoryTotal: status?.memory_total_gib,
  };
}

export default function LinuxDashboard() {
  const { status, connectionError, stale, ageSeconds, error, busy, autostart, native, refresh, act, clearError, setAutostartEnabled } = useLinuxBackend();
  const [activeTab, setActiveTab] = useState('overview');
  const [theme, setTheme] = useState<'mono' | 'cyber'>(() => localStorage.getItem('openrevo_theme') === 'cyber' ? 'cyber' : 'mono');

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.windowMode = 'full';
    localStorage.setItem('openrevo_theme', theme);
  }, [theme]);

  const exportDiagnostics = () => {
    if (!status) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(status, null, 2)], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url;
    link.download = 'openrevo-linux-diagnostics.json'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  const disabled = busy || !!connectionError || !!status?.control_error || stale || !status;
  const releaseDisabled = busy || !native;
  const controlProps = status ? { domains: status.domains, disabled, releaseDisabled, act } : null;
  const performance = controlProps && <PerformanceModes {...controlProps} />;
  const cooling = controlProps && <CoolingControls {...controlProps} />;
  const powerLimits = controlProps && <>
    {status?.domains.performance.limits && <CpuPowerControls {...controlProps} />}
    {!!status?.domains.performance.power_limits.length && <RevCard title="硬件功耗调校" icon={<Zap size={17} />}>
      {status.domains.performance.power_limits.map(limit => <PowerLimitControl key={limit.id} limit={limit} disabled={disabled}
        onApply={watts => act('power_limit', { id: limit.id, watts })} />)}
    </RevCard>}
  </>;
  const devices = status?.domains.devices;
  const battery = <RevCard title="电源与电池管理" icon={<BatteryCharging size={17} />}>
    {devices?.battery.supported && <EcBatteryControls devices={devices} disabled={disabled} act={act} />}
    {status?.batteries.length ? status.batteries.map(b => <section key={b.id} className="linux-stack">
      <div className="linux-reading"><span>{b.id} · {batteryNames[b.status ?? 'Unknown'] ?? b.status}</span><strong>{value(b.capacity, '%')}</strong></div>
      <div className="linux-reading"><span>健康度 / 当前功率</span><strong>{value(b.health_percent, '%', 1)} / {value(b.power_w, ' W', 1)}</strong></div>
      {devices?.battery.supported ? null : b.charge_limit == null ? <p className="linux-muted">此内核未提供限充接口。</p> : <>
        <label className="linux-reading">充电上限<select aria-label={`${b.id} 充电上限`} value={b.charge_limit} disabled={disabled || !b.charge_limit_writable}
          onChange={e => void act('battery_limit', { battery: b.id, limit: Number(e.target.value) })}>
          {[...new Set([50, 60, 70, 80, 90, 100, b.charge_limit])].sort((a, c) => a - c).map(n => <option key={n} value={n}>{n}%</option>)}</select></label>
        {!b.charge_limit_writable && <p className="linux-muted">限充接口只读，需要设备访问权限。</p>}</>}
    </section>) : <p className="linux-muted">未检测到电池。</p>}
  </RevCard>;
  const brightness = (kind: 'backlight' | 'leds') => <RevCard title={kind === 'backlight' ? '屏幕亮度' : '键盘背光亮度'} icon={<Monitor size={17} />}>
    {status?.backlights.some(b => b.kind === kind) ? status.backlights.filter(b => b.kind === kind).map(b =>
      <BrightnessControl key={b.id} id={b.id} percent={b.percent} disabled={disabled}
        onSet={percent => void act('backlight', { device: b.id, percent })} />) : <p className="linux-muted">内核未提供此类背光设备。</p>}
  </RevCard>;
  return <main className="acrylic-container openrevo-shell linux-shell" data-slot="shell" data-theme={theme}>
    <DashboardChrome theme={theme} onToggleTheme={() => setTheme(theme === 'mono' ? 'cyber' : 'mono')}
      versionLabel="LINUX" activeTab={activeTab} onSelectTab={setActiveTab}
      tabs={linuxTabs(status?.capabilities)}
      actions={<><button className="toggle-btn" onClick={() => void refresh()} aria-label="刷新"><RefreshCw size={14} /></button>
        <button className="toggle-btn" onClick={exportDiagnostics} disabled={!status}><Download size={14} />导出诊断</button></>} />
    <div className="shell-viewport" data-slot="viewport">
      <div className="linux-page-stack">
        {(error || connectionError || status?.control_error) && <div className="linux-error" role="alert">{error || connectionError || `后台控制异常：${status?.control_error}`}
          {error && <button className="toggle-btn" onClick={clearError}>关闭</button>}</div>}
        <div className="linux-status" data-stale={stale || !!connectionError}><Activity size={14} />{connectionError ? '连接中断 · 以下为上次数据' : stale ? `数据已过期 · ${ageSeconds} 秒未更新，暂停新调节，可恢复原策略` : status ? '已连接本机 · 每 2 秒更新' : '正在连接本机硬件…'}
          <span>{status?.model ?? '等待机型信息'}</span><span>{status?.ac_online == null ? '供电状态未知' : status.ac_online ? '外接电源' : '电池供电'}</span></div>
        <ModuleErrorBoundary key={activeTab} moduleName={activeTab} onNavigateToOverview={() => setActiveTab('overview')}>
          {activeTab === 'overview' && <div className="overview-slot-grid" data-slot="overview-grid">
            <div className="overview-slot-telemetry"><HardwareStatCards telemetry={telemetry(status)} /></div>
            <div className="overview-slot-power">{performance}</div>
            <div className="linux-two-columns">{cooling}{battery}</div>
            {devices && <HardwareSwitches devices={devices} disabled={disabled} releaseDisabled={releaseDisabled} act={act} status={status!} />}
          </div>}
          {activeTab === 'tuning' && <div className="linux-page-stack">{performance}
            <div className="linux-two-columns">{powerLimits}{cooling}</div>{controlProps && status?.domains.cooling.curve.supported && <OemFanCurve {...controlProps} />}{battery}</div>}
          {activeTab === 'gpu' && <div className="linux-page-stack">{brightness('backlight')}</div>}
          {activeTab === 'lighting' && <div className="linux-page-stack">{brightness('leds')}</div>}
          {activeTab === 'system' && <div className="linux-page-stack"><RevCard title="系统设置"><div className="sys-rows">
            <div className="quick-row"><div><div className="sys-row-label">登录时启动</div><div className="sys-row-desc">登录桌面后自动启动 OpenRevo</div></div>
              <label className="sys-row-controls"><input type="checkbox" aria-label="登录时启动" checked={autostart ?? false} disabled={busy || !native || autostart == null}
                onChange={e => void setAutostartEnabled(e.target.checked)} /></label></div>
            <div className="quick-row"><span className="sys-row-label">主板 / BIOS</span><span>{status?.board ?? '未提供'} / {status?.bios ?? '未提供'}</span></div>
            <div className="quick-row"><span className="sys-row-label">Linux 内核</span><span>{status?.kernel ?? '未提供'}</span></div>
            <div className="quick-row"><span className="sys-row-label">EC 控制协议</span><span>{!status?.oem.available ? '未接入' : status.oem.control_abi == null ? '需更新驱动 · 当前仅监测' : `ABI ${status.oem.control_abi}${status.oem.control_fault ? ' · 正在恢复故障' : ''}`}</span></div>
          </div></RevCard><RevCard title="硬件支持状态" subtitle="接口存在与当前进程有写入权限分别检查">
            <div className="linux-capabilities">{status?.capabilities.map(c => <div key={c.id} className="linux-capability">
              <span className={c.available ? 'linux-available' : 'linux-muted'}>{c.available ? '● 已检测到' : '○ 未接入'}</span><strong>{c.name}</strong><p>{c.detail}</p>
            </div>)}</div></RevCard><RevCard title="全部传感器"><div className="linux-sensor-grid">{status?.sensors.map(s =>
              <div className="sys-sub-panel" key={s.id}><div className="linux-reading"><span>{s.device} · {s.label}</span><strong>{value(s.value, ` ${s.unit}`, 1)}</strong></div>
                {s.source && <p className="sys-row-desc linux-sensor-source">{readingDescription(s)}</p>}</div>)}</div>
              <TelemetrySources status={status} /></RevCard></div>}
          {activeTab === 'sponsor' && <SponsorView />}
        </ModuleErrorBoundary>
      </div>
    </div>
  </main>;
}
