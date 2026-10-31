import { useCallback, useEffect, useRef, useState } from 'react';
import { invoke, isTauri } from '@tauri-apps/api/core';
import { Activity, BatteryCharging, Check, Cpu, Download, Fan, Monitor, Moon, RefreshCw, Sun, Zap } from 'lucide-react';
import { RevCard } from './ui/RevCard';
import { LinuxSnapshot } from '../types/linux';
import logo from '../assets/logo.png';

const profileNames: Record<string, string> = {
  'power-saver': '节能', balanced: '均衡', performance: '性能',
};
const batteryNames: Record<string, string> = {
  Charging: '充电中', Discharging: '使用电池', Full: '已充满',
  'Not charging': '未充电', Unknown: '状态未知',
};
const value = (n: number | null | undefined, unit = '', digits = 0) =>
  n == null ? '未提供' : `${n.toFixed(digits)}${unit}`;

function BrightnessControl({ id, percent, disabled, onSet }: {
  id: string; percent: number | null; disabled: boolean; onSet: (percent: number) => void;
}) {
  const [draft, setDraft] = useState(percent ?? 50);
  const editing = useRef(false);
  useEffect(() => { if (!editing.current && percent != null) setDraft(percent); }, [percent]);
  return <label className="linux-brightness">
    <span>{id}</span>
    <input aria-label={`${id} 亮度`} type="range" min="5" max="100" value={draft} disabled={disabled}
      onChange={e => { editing.current = true; setDraft(Number(e.target.value)); }}
      onPointerUp={() => { editing.current = false; onSet(draft); }}
      onKeyUp={e => { if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(e.key)) {
        editing.current = false; onSet(draft);
      } }}
      onBlur={() => { editing.current = false; }} />
    <output>{draft}%</output>
  </label>;
}

export default function LinuxDashboard() {
  const [status, setStatus] = useState<LinuxSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [stale, setStale] = useState(false);
  const [autostart, setAutostart] = useState(false);
  const [theme, setTheme] = useState(() => localStorage.getItem('openrevo_theme') || 'mono');
  const inFlight = useRef(false);
  const mounted = useRef(true);
  const native = isTauri();

  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document.documentElement.dataset.windowMode = 'full';
    localStorage.setItem('openrevo_theme', theme);
  }, [theme]);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    if (!native) {
      setError('请通过 pnpm tauri dev 启动 Linux 桌面程序，浏览器预览无法读取本机硬件。');
      return;
    }
    inFlight.current = true;
    try {
      const next = await invoke<LinuxSnapshot>('get_linux_status');
      if (mounted.current) { setStatus(next); setStale(false); }
    } catch (e) {
      if (mounted.current) { setError(String(e)); setStale(true); }
    } finally { inFlight.current = false; }
  }, [native]);

  useEffect(() => {
    mounted.current = true;
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      await refresh();
      if (!cancelled) timer = setTimeout(poll, 2000);
    };
    void poll();
    if (native) invoke<boolean>('get_autostart_enabled').then(v => {
      if (mounted.current) setAutostart(v);
    }).catch(() => {});
    return () => { cancelled = true; mounted.current = false; clearTimeout(timer); };
  }, [native, refresh]);

  const act = async (command: string, args: Record<string, unknown>) => {
    setBusy(true); setError(null);
    try { await invoke(command, args); await refresh(); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const exportDiagnostics = () => {
    if (!status) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(status, null, 2)], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url;
    link.download = 'openrevo-linux-diagnostics.json'; link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };

  const temperatures = status?.sensors.filter(s => s.kind === 'temperature') ?? [];
  const fans = status?.sensors.filter(s => s.kind === 'fan') ?? [];
  const powers = status?.sensors.filter(s => s.kind === 'power') ?? [];
  const disabled = busy || stale;

  return <main className="acrylic-container linux-shell" data-theme={theme}>
    <header className="linux-header">
      <div className="linux-brand"><img src={logo} alt="" /><div><h1>OPENREVO <span>LINUX · 开发版</span></h1>
        <p>{status?.model ?? '等待硬件信息'} · {status?.kernel ?? 'Linux'}</p></div></div>
      <div className="linux-actions">
        <button className="toggle-btn" aria-label="切换主题" onClick={() => setTheme(theme === 'mono' ? 'cyber' : 'mono')}>
          {theme === 'mono' ? <Sun size={16} /> : <Moon size={16} />}</button>
        <button className="toggle-btn" onClick={() => void refresh()} aria-label="刷新"><RefreshCw size={16} /></button>
        <button className="toggle-btn" onClick={exportDiagnostics} disabled={!status}><Download size={15} />导出诊断</button>
      </div>
    </header>

    <div className="linux-content">
      {error && <div className="linux-error" role="alert">{error}<button className="toggle-btn" onClick={() => setError(null)}>关闭</button></div>}
      <div className="linux-status"><Activity size={14} />{stale ? '连接中断 · 以下为上次数据' : status ? '已连接本机 · 每 2 秒更新' : '正在连接本机硬件…'}
        <span>{status?.ac_online == null ? '供电状态未知' : status.ac_online ? '外接电源' : '电池供电'}</span></div>

      <div className="linux-stats">
        <RevCard title="CPU" subtitle={status?.cpu ?? '未提供'} icon={<Cpu size={17} />}>
          <strong className="linux-stat-value">{value(status?.cpu_usage, '%')}</strong>
          <span className="linux-muted">平均频率 {value(status?.cpu_frequency_mhz, ' MHz')}</span>
        </RevCard>
        <RevCard title="内存" icon={<Activity size={17} />}>
          <strong className="linux-stat-value">{value(status?.memory_used_gib, ' GiB', 1)}</strong>
          <span className="linux-muted">共 {value(status?.memory_total_gib, ' GiB', 1)}</span>
        </RevCard>
        <RevCard title="风扇" icon={<Fan size={17} />}>
          {fans.length ? fans.map(s => <div className="linux-reading" key={s.id}><span>{s.device} · {s.label}</span><strong>{value(s.value, ' RPM')}</strong></div>) :
            <><strong className="linux-stat-value">未提供</strong><span className="linux-muted">内核未公开风扇转速</span></>}
        </RevCard>
      </div>

      <RevCard title="系统性能档位" subtitle="使用系统电源服务，与桌面设置保持同步" icon={<Zap size={17} />}>
        <div className="linux-profile-options">{['power-saver', 'balanced', 'performance'].map(p =>
          <button key={p} className={`toggle-btn ${status?.power.active === p ? 'active-indigo' : ''}`}
            aria-pressed={status?.power.active === p}
            disabled={disabled || status?.power.can_set === false || !status?.power.available.includes(p)}
            onClick={() => void act('set_linux_power_profile', { profile: p })}>
            {status?.power.active === p && <Check size={14} />}{profileNames[p]}</button>)}</div>
        <p className="linux-muted">系统档位不代表机身 EC 档位，当前尚未联动机身模式灯、风扇或 OEM 功耗预设。</p>
        {status?.power.error && <p className="linux-warning">{status.power.error}</p>}
      </RevCard>

      <div className="linux-two-columns">
        <RevCard title="温度与功耗" subtitle="Linux hwmon 实时测量" icon={<Cpu size={17} />}>
          {temperatures.length + powers.length ? [...temperatures, ...powers].map(s =>
            <div className="linux-reading" key={s.id}><span>{s.device} · {s.label}</span><strong>{value(s.value, ` ${s.unit}`, 1)}</strong></div>) :
            <p className="linux-muted">内核未提供可读取的传感器。</p>}
        </RevCard>

        <div className="linux-stack">
          <RevCard title="电池与限充" icon={<BatteryCharging size={17} />}>
            {status?.batteries.length ? status.batteries.map(b => <section key={b.id} className="linux-stack">
              <div className="linux-reading"><span>{b.id} · {batteryNames[b.status ?? 'Unknown'] ?? b.status}</span><strong>{value(b.capacity, '%')}</strong></div>
              <div className="linux-reading"><span>健康度 / 当前功率</span><strong>{value(b.health_percent, '%', 1)} / {value(b.power_w, ' W', 1)}</strong></div>
              {b.charge_limit == null ? <p className="linux-muted">此内核未提供限充接口。</p> : <>
                <label className="linux-reading">充电上限<select aria-label={`${b.id} 充电上限`} value={b.charge_limit}
                  disabled={disabled || !b.charge_limit_writable}
                  onChange={e => void act('set_linux_battery_limit', { battery: b.id, limit: Number(e.target.value) })}>
                  {[...new Set([50, 60, 70, 80, 90, 100, b.charge_limit])].sort((a, c) => a - c).map(n => <option key={n} value={n}>{n}%</option>)}</select></label>
                {!b.charge_limit_writable && <p className="linux-muted">限充接口只读，需要设备访问权限。</p>}
              </>}
            </section>) : <p className="linux-muted">未检测到电池。</p>}
          </RevCard>
          <RevCard title="亮度" subtitle="通过当前桌面会话设置" icon={<Monitor size={17} />}>
            {status?.backlights.length ? status.backlights.map(b =>
              <BrightnessControl key={b.id} id={b.id} percent={b.percent} disabled={disabled}
                onSet={percent => void act('set_linux_backlight', { device: b.id, percent })} />) :
              <p className="linux-muted">内核未提供背光设备。</p>}
          </RevCard>
        </div>
      </div>

      <RevCard title="硬件支持状态" subtitle="以当前 Linux 接口为准；尚未移植的功能保持不可用">
        <div className="linux-capabilities">{status?.capabilities.map(c => <div key={c.id} className="linux-capability">
          <span className={c.available ? 'linux-available' : 'linux-muted'}>{c.available ? '● 可用' : '○ 未接入'}</span>
          <strong>{c.name}</strong><p>{c.detail}</p>
        </div>)}</div>
      </RevCard>
      <footer className="linux-footer"><span>主板 {status?.board ?? '未提供'} · BIOS {status?.bios ?? '未提供'}</span>
        <label><input type="checkbox" checked={autostart} disabled={busy || !native} onChange={async e => {
          const enabled = e.target.checked; setBusy(true); setError(null);
          try { await invoke('set_autostart_enabled', { enabled }); setAutostart(enabled); }
          catch (e) { setError(String(e)); } finally { setBusy(false); }
        }} />登录时启动</label></footer>
    </div>
  </main>;
}
