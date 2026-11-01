import { useEffect, useState } from 'react';
import { Fan, Zap } from 'lucide-react';
import { RevCard } from '../ui/RevCard';
import { ModeSelector } from '../ModeSelector';
import { CpuPlSliderRow } from '../tuning/CpuPlSliderRow';
import { CpuLimits, LinuxOemSnapshot } from '../../types/linux';
import type { LinuxAction } from '../../lib/linuxApi';

export interface OemControlProps {
  oem: LinuxOemSnapshot;
  disabled: boolean;
  acOnline: boolean;
  act: LinuxAction;
}

export function OemModes({ oem, disabled, acOnline, act }: OemControlProps) {
  const onAdapter = acOnline && oem.dc_jack;
  return <RevCard title="性能释放模式" subtitle="机身 EC · Uniwill WMI" icon={<Zap size={17} />}>
    <ModeSelector currentMode={oem.mode ?? 0} disabled={disabled || !oem.writable}
      capabilities={{ power_modes: ['office', 'balance', 'turbo'] }}
      isDcOnline={onAdapter} unlockBatteryLimit={false}
      unavailableModes={!onAdapter ? { 2: '需要连接原装电源', 3: '需要连接原装电源' } : {}}
      onSelectMode={mode => void act('set_linux_oem_mode', { mode })} />
    <p className="linux-muted">{oem.fan_curve?.active ? '自定义曲线运行中；切换性能模式会先恢复原风扇策略。' : oem.custom_power ? 'CPU 使用自定义功耗；选择性能模式可恢复出厂功耗。' : '使用 EC 出厂功耗与自动风扇策略，切换后读取机身模式确认。'}</p>
    {!onAdapter && <p className="linux-muted">电池供电：可切换办公模式，均衡/狂暴与功耗调节需要原装电源。</p>}
    {!oem.writable && <p className="linux-warning">EC 通道处于只读模式。</p>}
    {oem.error && <p className="linux-warning">{oem.error}</p>}
  </RevCard>;
}

export function OemCooling({ oem, disabled, act }: OemControlProps) {
  const protectedBoost = !!oem.fan_curve?.fault && oem.fan_boost;
  return <RevCard title="风扇与散热控制" subtitle="原版 EC 强冷控制" icon={<Fan size={17} />}>
    {oem.fan_rpm.map((rpm, index) => <div className="linux-reading" key={index}>
      <span>{index === 0 ? 'CPU 风扇' : 'GPU 风扇'}</span><strong>{rpm} RPM</strong>
    </div>)}
    <div className="quick-row"><div><div className="sys-row-label">一键强冷</div>
      <div className="sys-row-desc">{oem.fan_curve?.fault ? '风扇策略恢复失败，恢复成功前无法关闭强冷。' : oem.fan_boost ? '两只风扇强冷运行中' : 'EC 根据温度自动调速'}</div></div>
      <button className={`toggle-btn ${oem.fan_boost ? 'active-cyan' : ''}`} aria-pressed={oem.fan_boost}
        disabled={disabled || !oem.writable || protectedBoost} onClick={() => void act('set_linux_oem_fan_boost', { enabled: !oem.fan_boost })}>
        {oem.fan_boost ? '恢复自动' : '一键强冷'}</button></div>
    <p className="linux-muted">退出或界面失联后恢复开启前的强冷状态。可在电源管理中编辑自定义曲线。</p>
  </RevCard>;
}

export function OemPower({ oem, disabled, acOnline, act }: OemControlProps) {
  const [draft, setDraft] = useState<CpuLimits | null>(oem.limits);
  const [dirty, setDirty] = useState(false);
  useEffect(() => { if (!dirty) setDraft(oem.limits); }, [oem.limits?.pl1, oem.limits?.pl2, oem.limits?.pl4, dirty]);
  const ceiling = oem.presets[2];
  const locked = disabled || !oem.writable || !acOnline || !oem.dc_jack || !!oem.fan_curve?.active;
  const valid = draft && ceiling && [draft.pl1, draft.pl2, draft.pl4].every(Number.isInteger)
    && draft.pl1 >= 15 && draft.pl1 <= draft.pl2 && draft.pl4 >= draft.pl2 + 20
    && draft.pl1 <= ceiling.pl1 && draft.pl2 <= ceiling.pl2 && draft.pl4 <= ceiling.pl4;
  return <RevCard title="CPU 功耗调校" subtitle="PL1 / PL2 / PL4 · EC 出厂上限" icon={<Zap size={17} />}>
    {draft && ceiling && <section className="sys-sub-panel linux-stack">
      {(['pl1', 'pl2', 'pl4'] as const).map(key => <CpuPlSliderRow key={key}
        label={{ pl1: 'PL1 持续功耗', pl2: 'PL2 短时功耗', pl4: 'PL4 瞬时上限' }[key]}
        value={draft[key]} min={15} max={ceiling[key]} step={1} disabled={locked}
        onChange={n => { setDraft({ ...draft, [key]: n }); setDirty(true); }} />)}
      <div className="linux-control-actions"><button className="toggle-btn active-indigo" disabled={locked || !dirty || !valid}
        onClick={async () => { if (await act('set_linux_oem_cpu_limits', { ...draft })) setDirty(false); }}>应用 CPU 功耗</button>
        <button className="toggle-btn" disabled={disabled || !dirty} onClick={() => { setDraft(oem.limits); setDirty(false); }}>还原输入</button></div>
      {dirty && !valid && <p className="linux-warning">要求 PL1 ≤ PL2、PL4 ≥ PL2 + 20 W，且不超过出厂上限。</p>}
    </section>}
    <p className="linux-muted">{oem.custom_power ? '当前自定义功耗读回' : '当前机身模式的出厂功耗表'}：{oem.limits ? `${oem.limits.pl1} / ${oem.limits.pl2} / ${oem.limits.pl4} W` : '未提供'}。</p>
    <p className="linux-muted">保留原版 PL4 瞬时余量规则，三个限值一起校验并应用。功耗表读回不代表 CPU 实际耗电。</p>
    {oem.fan_curve?.active ? <p className="linux-muted">请先恢复原风扇策略，再调整 CPU 功耗。</p> : locked && <p className="linux-muted">连接原装电源并启用 EC 写入后可调节。</p>}
  </RevCard>;
}
