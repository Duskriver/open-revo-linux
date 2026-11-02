import { useEffect, useState } from 'react';
import { RevCard } from '../ui/RevCard';
import type { LinuxAction } from '../../lib/linuxApi';
import type { DeviceSwitch, LinuxDomains, LinuxSnapshot } from '../../types/linux';

type Props = { devices: LinuxDomains['devices']; disabled: boolean; act: LinuxAction };
export function EcBatteryControls({ devices, disabled, act }: Props) {
  const battery = devices.battery;
  const [lower, setLower] = useState(battery.lower ?? 75);
  const [upper, setUpper] = useState(battery.upper ?? 80);
  const [dirty, setDirty] = useState(false);
  useEffect(() => { if (!dirty) { setLower(battery.lower ?? 75); setUpper(battery.upper ?? 80); } }, [battery.lower, battery.upper, dirty]);
  const locked = disabled || !battery.writable;
  const valid = Number.isInteger(lower) && Number.isInteger(upper) && lower >= 1 && lower < upper && upper <= 100;
  return <section className="sys-sub-panel linux-stack">
    <div className="sys-row-label">EC 电池养护</div>
    <div className="sys-row-desc">充电低于下限时恢复、达到上限时停止；由 EC 固件执行。</div>
    <div className="linux-control-actions">{(['full', 'balanced', 'longevity'] as const).map(mode =>
      <button key={mode} className={`toggle-btn ${battery.mode === mode ? 'active-emerald' : ''}`} aria-pressed={battery.mode === mode} disabled={locked}
        onClick={async () => { if (await act('battery', { policy: { mode, lower: null, upper: null } })) setDirty(false); }}>
        {{ full: '满充 100%', balanced: '均衡 80%', longevity: '长效 60%' }[mode]}</button>)}</div>
    <div className="quick-row"><div><div className="sys-row-label">自定义双阈值</div><div className="sys-row-desc">当前读回：{battery.lower ?? '未知'}% → {battery.upper ?? '未知'}%</div></div>
      <div className="sys-row-controls"><label>下限 <input className="device-threshold" aria-label="EC 充电下限" type="number" min="1" max="99" value={lower} disabled={locked} onChange={e => { setLower(Number(e.target.value)); setDirty(true); }} /></label>
        <label>上限 <input className="device-threshold" aria-label="EC 充电上限" type="number" min="2" max="100" value={upper} disabled={locked} onChange={e => { setUpper(Number(e.target.value)); setDirty(true); }} /></label>
        <button className="toggle-btn active-indigo" disabled={locked || !dirty || !valid} onClick={async () => { if (await act('battery', { policy: { mode: 'custom', lower, upper } })) setDirty(false); }}>应用阈值</button>
      </div></div>
    {dirty && !valid && <p className="linux-warning">下限必须小于上限，范围为 1–100%。</p>}
    {(battery.error || devices.error) && <p className="linux-warning">{battery.error || devices.error}</p>}
  </section>;
}
const labels: Record<DeviceSwitch, string> = { win_lock: 'Win 键锁定', fn_lock: 'Fn 锁定', camera: '摄像头供电', usb_charge: '关机 USB 充电', ac_recovery: '来电自启', touchpad: '触摸板 EC 标记' };
export function HardwareSwitches({ devices, disabled, act, status, releaseDisabled }: Props & { status: LinuxSnapshot; releaseDisabled: boolean }) {
  return <RevCard title="硬件快捷开关"><div className="sys-rows">
    {devices.switches.map(item => <div key={item.id} className="quick-row"><div><div className="sys-row-label">{labels[item.id]}</div><div className="sys-row-desc">{item.detail}</div></div>
      <button className={`toggle-btn ${item.enabled ? 'active-cyan' : ''} ${item.id === 'camera' && item.enabled ? 'danger' : ''}`} disabled={disabled || !item.writable || item.enabled == null} aria-label={labels[item.id]} aria-pressed={item.enabled ?? false}
        onClick={() => void act('switch', { switch: item.id, enabled: !item.enabled })}>{item.enabled == null ? '未知' : item.enabled ? '已开启' : '已关闭'}</button></div>)}
    <div className="quick-row"><div><div className="sys-row-label">机身模式键联动</div><div className="sys-row-desc">办公 → 均衡 → 狂暴 → 自定义；跳过当前不可用档位，发送桌面通知。</div></div>
      <button className={`toggle-btn ${status.hotkeys_enabled ? 'active-indigo' : ''}`} aria-label="机身模式键联动" aria-pressed={status.hotkeys_enabled ?? false} disabled={status.hotkeys_enabled ? releaseDisabled : disabled || !status.wmi_events?.available}
        onClick={() => void act('hotkeys', { enabled: !status.hotkeys_enabled })}>{status.hotkeys_enabled ? '已启用' : '未启用'}</button></div>
  </div>{(devices.error || status.hotkey_error) && <p className="linux-warning">{devices.error || status.hotkey_error}</p>}
    {!status.wmi_events?.available && <p className="linux-muted">{status.wmi_events?.error ?? '机身事件通道不可用'}</p>}
  </RevCard>;
}
