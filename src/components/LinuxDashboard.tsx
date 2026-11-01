import { useEffect, useRef, useState } from 'react';
import { Activity, BatteryCharging, Download, Fan, Monitor, RefreshCw, Sliders, Zap } from 'lucide-react';
import { RevCard } from './ui/RevCard';
import { DashboardChrome } from './common/DashboardChrome';
import { HardwareStatCards, HardwareTelemetry } from './overview/HardwareStatCards';
import { ModeSelector } from './ModeSelector';
import { CpuPlSliderRow } from './tuning/CpuPlSliderRow';
import { ModuleErrorBoundary } from './ModuleErrorBoundary';
import { SponsorView } from './SponsorView';
import { useLinuxBackend } from '../hooks/useLinuxBackend';
import { LINUX_TABS } from './linux/navigation';
import { LinuxPowerLimit, LinuxSnapshot } from '../types/linux';
import { OemFanCurve } from './linux/OemFanCurve';
import { OemCooling, OemModes, OemPower } from './linux/OemHardwareControls';
import { readingDescription, TelemetrySources } from './linux/TelemetrySources';
import type { PowerMode } from '../types/controls';

const profileNames: Record<string, string> = {
  'power-saver': '节能', 'low-power': '节能', quiet: '静音', cool: '低温', balanced: '均衡',
  'balanced-performance': '均衡性能', performance: '性能', 'max-power': '最高性能', custom: '自定义',
};
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
  const fans = status?.sensors.filter(s => s.kind === 'fan') ?? [];
  const gpu = status?.sensors.find(s => s.kind === 'temperature' && /amdgpu|nouveau|nvidia/i.test(s.device));
  return {
    cpuName: status?.cpu, cpuTemp: sensor('temperature', /k10temp|coretemp|zenpower/i),
    cpuUsage: status?.cpu_usage, cpuFrequency: status?.cpu_frequency_mhz,
    cpuPower: sensor('power', /k10temp|coretemp|zenpower|rapl/i),
    gpuName: gpu ? `${gpu.device} · ${gpu.label}` : null, gpuTemp: gpu?.value,
    gpuPower: sensor('power', /amdgpu|nouveau|nvidia/i),
    fanRpm: status?.oem.available ? status.oem.fan_rpm : fans.map(s => s.value), fanBoost: status?.oem.available ? status.oem.fan_boost : status?.controls.fans.some(f => f.boosted),
    fanState: status?.oem.available ? (status.oem.fan_boost ? '一键强冷' : 'EC 自动调速') : status?.controls.fans.some(f => f.boosted) ? '一键强冷' : status?.controls.fans.length ? '驱动控制' : '未公开调速接口',
    memoryLabel: '系统内存', memoryUsed: status?.memory_used_gib, memoryTotal: status?.memory_total_gib,
  };
}

export default function LinuxDashboard() {
  const { status, connectionError, leaseError, stale, ageSeconds, error, busy, autostart, native, refresh, act, clearError, setAutostartEnabled } = useLinuxBackend();
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
  const disabled = busy || !!connectionError || !!leaseError || stale || !status;
  const releaseDisabled = busy || !native;
  const platform = status?.controls.platform;
  const hasPlatform = !!platform?.available.length;
  const servicePresent = !!status?.power.available.length;
  const profiles = hasPlatform ? platform!.available : status?.power.available ?? [];
  const activeProfile = hasPlatform ? platform!.active : status?.power.active;
  const standardProfiles: Record<PowerMode, string> = { 1: hasPlatform ? 'low-power' : 'power-saver', 2: 'balanced', 3: 'performance', 4: 'custom' };
  const mode = ([1, 2, 3] as PowerMode[]).find(id => standardProfiles[id] === activeProfile) ?? 0;
  const canSelect = (profile: string) => profiles.includes(profile) && (servicePresent
    ? status?.power.can_set !== false && status?.power.available.includes(profile === 'low-power' ? 'power-saver' : profile)
    : !!platform?.writable);
  const chooseProfile = (profile: string) => void act(hasPlatform ? 'set_linux_platform_profile' : 'set_linux_power_profile', { profile });
  const oemProps = status ? { oem: status.oem, disabled, releaseDisabled, acOnline: status.ac_online === true, act } : null;
  const performance = status?.oem.available && oemProps ? <OemModes {...oemProps} /> : <RevCard title="性能释放模式" subtitle={hasPlatform ? `平台性能 · ${platform?.source}` : '系统性能档位'} icon={<Zap size={17} />}>
    <div className="linux-profile-options"><ModeSelector currentMode={mode} onSelectMode={id => chooseProfile(standardProfiles[id])}
      disabled={disabled} unlockBatteryLimit capabilities={{ power_modes: ['office', 'balance', 'turbo'] }}
      unavailableModes={Object.fromEntries(([1, 2, 3] as PowerMode[]).filter(id => !canSelect(standardProfiles[id])).map(id => [id, '驱动未提供此档位或当前进程没有切换权限']))}
      modeLabels={{ 1: '节能 / 办公', 2: '均衡', 3: '性能' }} /></div>
    {profiles.filter(p => !Object.values(standardProfiles).includes(p)).map(p => <button key={p}
      className={`toggle-btn ${activeProfile === p ? 'active-indigo' : ''}`} disabled={disabled || !canSelect(p)} onClick={() => chooseProfile(p)}>{profileNames[p] ?? p}</button>)}
    <div className="linux-control-actions"><span>当前模式：{activeProfile ? profileNames[activeProfile] ?? activeProfile : '未提供'}</span>
      <button className="toggle-btn" onClick={() => setActiveTab('tuning')}><Sliders size={14} />功耗与风扇调校</button></div>
    <p className="linux-muted">{hasPlatform ? '切换后确认平台模式读回；机身 EC 模式灯联动尚未接入。' : '当前仅提供系统电源策略；内核未公开平台性能模式。'}</p>
    {status?.power.error && <p className="linux-warning">{status.power.error}</p>}
  </RevCard>;
  const cooling = status?.oem.available && oemProps ? <OemCooling {...oemProps} /> : <RevCard title="风扇与散热控制" icon={<Fan size={17} />}>
    {status?.controls.fans.length ? status.controls.fans.map(fan => <div className="quick-row" key={fan.id}>
      <div><div className="sys-row-label">{fan.name} · {value(fan.rpm, ' RPM')}</div>
        <div className="sys-row-desc">{fan.reason ?? (fan.boosted ? '强冷运行中，关闭后恢复原自动模式' : '由驱动自动调速')}</div></div>
      <div className="sys-row-controls"><button className={`toggle-btn ${fan.boosted ? 'active-cyan' : ''}`}
        disabled={(fan.boosted ? releaseDisabled : disabled) || !fan.writable} aria-pressed={fan.boosted}
        onClick={() => void act('set_linux_fan_boost', { id: fan.id, enabled: !fan.boosted })}>{fan.boosted ? '恢复自动' : '一键强冷'}</button></div>
    </div>) : <div className="sys-sub-panel"><div className="sys-row-label">未检测到风扇控制驱动</div>
      <p className="sys-row-desc">当前固件继续管理散热。需要机型驱动公开风扇控制接口后，才能使用一键强冷。</p></div>}
    <p className="linux-muted">自定义风扇曲线尚待机型适配。强冷在界面失联后自动释放，正常退出时恢复原自动控制模式。</p>
    {status?.controls.fan_error && <p className="linux-warning" role="alert">{status.controls.fan_error}</p>}
  </RevCard>;
  const powerLimits = status?.oem.available && oemProps ? <OemPower {...oemProps} /> : <RevCard title="硬件功耗调校" subtitle="调节上限以设备驱动报告为准" icon={<Zap size={17} />}>
    {status?.controls.power_limits.length ? status.controls.power_limits.map(limit =>
      <PowerLimitControl key={limit.id} limit={limit} disabled={disabled}
        onApply={watts => act('set_linux_power_limit', { id: limit.id, watts })} />) :
      <div className="sys-sub-panel"><div className="sys-row-label">未检测到可调节的功耗接口</div>
        <p className="sys-row-desc">当前内核没有公开 CPU / GPU 功耗上限；上方性能模式仍可使用已检测到的档位。</p></div>}
  </RevCard>;
  const battery = <RevCard title="电源与电池管理" icon={<BatteryCharging size={17} />}>
    {status?.batteries.length ? status.batteries.map(b => <section key={b.id} className="linux-stack">
      <div className="linux-reading"><span>{b.id} · {batteryNames[b.status ?? 'Unknown'] ?? b.status}</span><strong>{value(b.capacity, '%')}</strong></div>
      <div className="linux-reading"><span>健康度 / 当前功率</span><strong>{value(b.health_percent, '%', 1)} / {value(b.power_w, ' W', 1)}</strong></div>
      {b.charge_limit == null ? <p className="linux-muted">此内核未提供限充接口。</p> : <>
        <label className="linux-reading">充电上限<select aria-label={`${b.id} 充电上限`} value={b.charge_limit} disabled={disabled || !b.charge_limit_writable}
          onChange={e => void act('set_linux_battery_limit', { battery: b.id, limit: Number(e.target.value) })}>
          {[...new Set([50, 60, 70, 80, 90, 100, b.charge_limit])].sort((a, c) => a - c).map(n => <option key={n} value={n}>{n}%</option>)}</select></label>
        {!b.charge_limit_writable && <p className="linux-muted">限充接口只读，需要设备访问权限。</p>}</>}
    </section>) : <p className="linux-muted">未检测到电池。</p>}
  </RevCard>;
  const brightness = (kind: 'backlight' | 'leds') => <RevCard title={kind === 'backlight' ? '屏幕亮度' : '键盘背光亮度'} icon={<Monitor size={17} />}>
    {status?.backlights.some(b => b.kind === kind) ? status.backlights.filter(b => b.kind === kind).map(b =>
      <BrightnessControl key={b.id} id={b.id} percent={b.percent} disabled={disabled}
        onSet={percent => void act('set_linux_backlight', { device: b.id, percent })} />) : <p className="linux-muted">内核未提供此类背光设备。</p>}
  </RevCard>;
  const unavailable: Record<string, [string, string]> = {
    gpu: ['显卡模式', '独显直连 / 混合输出和 GPU 超频仍待 Linux 机型接口适配。已发现的硬件功耗控制位于“电源管理”。'],
    lighting: ['RGB 灯效工坊', 'RGB 多色灯效、光带和原版灯效引擎仍待 Linux HID 传输层适配。'],
    water: ['水冷魔盒 (BLE)', '水冷设备发现、连接和调节仍待 BlueZ 后端适配。'],
    plugins: ['插件', 'Linux 插件运行环境和权限隔离尚未接通。'],
  };
  return <main className="acrylic-container openrevo-shell linux-shell" data-slot="shell" data-theme={theme}>
    <DashboardChrome theme={theme} onToggleTheme={() => setTheme(theme === 'mono' ? 'cyber' : 'mono')}
      versionLabel="LINUX" activeTab={activeTab} onSelectTab={setActiveTab}
      tabs={LINUX_TABS}
      actions={<><button className="toggle-btn" onClick={() => void refresh()} aria-label="刷新"><RefreshCw size={14} /></button>
        <button className="toggle-btn" onClick={exportDiagnostics} disabled={!status}><Download size={14} />导出诊断</button></>} />
    <div className="shell-viewport" data-slot="viewport">
      <div className="linux-page-stack">
        {(error || connectionError || leaseError) && <div className="linux-error" role="alert">{error || connectionError || `控制续租失败：${leaseError}`}
          {error && <button className="toggle-btn" onClick={clearError}>关闭</button>}</div>}
        <div className="linux-status" data-stale={stale || !!connectionError}><Activity size={14} />{connectionError ? '连接中断 · 以下为上次数据' : stale ? `数据已过期 · ${ageSeconds} 秒未更新，暂停新调节，可恢复原策略` : status ? '已连接本机 · 每 2 秒更新' : '正在连接本机硬件…'}
          <span>{status?.model ?? '等待机型信息'}</span><span>{status?.ac_online == null ? '供电状态未知' : status.ac_online ? '外接电源' : '电池供电'}</span></div>
        <ModuleErrorBoundary key={activeTab} moduleName={activeTab} onNavigateToOverview={() => setActiveTab('overview')}>
          {activeTab === 'overview' && <div className="overview-slot-grid" data-slot="overview-grid">
            <div className="overview-slot-telemetry"><HardwareStatCards telemetry={telemetry(status)} /></div>
            <div className="overview-slot-power">{performance}</div>
            <div className="linux-two-columns">{cooling}{battery}</div>
          </div>}
          {activeTab === 'tuning' && <div className="linux-page-stack">{performance}<div className="linux-two-columns">{powerLimits}{cooling}</div>{oemProps && <OemFanCurve {...oemProps} />}{battery}</div>}
          {unavailable[activeTab] && <div className="linux-page-stack"><RevCard title={unavailable[activeTab][0]}>
            <div className="sys-sub-panel"><div className="sys-row-label">此功能尚待移植</div><p className="sys-row-desc">{unavailable[activeTab][1]}</p></div>
          </RevCard>{activeTab === 'gpu' && brightness('backlight')}{activeTab === 'lighting' && brightness('leds')}</div>}
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
