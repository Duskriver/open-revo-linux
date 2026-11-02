import { useEffect, useState } from 'react';
import { Fan, Zap } from 'lucide-react';
import { RevCard } from '../ui/RevCard';
import { ModeSelector } from '../ModeSelector';
import { CpuPlSliderRow } from '../tuning/CpuPlSliderRow';
import { CpuLimits, LinuxDomains } from '../../types/linux';
import type { LinuxAction } from '../../lib/linuxApi';

export interface DomainControlProps {
  domains: LinuxDomains;
  disabled: boolean;
  releaseDisabled: boolean;
  act: LinuxAction;
}

const modeIds = { office: 1, balanced: 2, turbo: 3, custom: 4 } as const;
const modes = ['office', 'balanced', 'turbo', 'custom'] as const;
export function PerformanceModes({ domains, disabled, act }: DomainControlProps) {
  const performance = domains.performance;
  return <RevCard title="性能释放模式" icon={<Zap size={17} />}>
    <ModeSelector currentMode={performance.active ? modeIds[performance.active] : 0} disabled={disabled}
      capabilities={{ power_modes: ['office', 'balance', 'turbo', 'custom'] }} unlockBatteryLimit
      unavailableModes={Object.fromEntries(performance.modes.filter(m => !m.enabled).map(m => [modeIds[m.mode], m.reason ?? '当前不可用']))}
      onSelectMode={id => void act('mode', { mode: modes[id - 1] })} />
    <p className="linux-muted">切换后自动保存设置；自定义模式使用已保存的 CPU 功耗参数。</p>
    {performance.error && <p className="linux-warning">{performance.error}</p>}
  </RevCard>;
}

export function CoolingControls({ domains, disabled, releaseDisabled, act }: DomainControlProps) {
  const cooling = domains.cooling;
  return <RevCard title="风扇与散热控制" icon={<Fan size={17} />}>
    {cooling.rpm.map((rpm, index) => <div className="linux-reading" key={index}>
      <span>风扇 {index + 1}</span><strong>{rpm == null ? '未提供' : `${rpm} RPM`}</strong>
    </div>)}
    <div className="quick-row"><div><div className="sys-row-label">一键强冷</div>
      <div className="sys-row-desc">{cooling.boosted ? '强冷运行中' : '自动调速'}</div></div>
      <button className={`toggle-btn ${cooling.boosted ? 'active-cyan' : ''}`} aria-pressed={cooling.boosted}
        disabled={(cooling.boosted ? releaseDisabled : disabled) || !cooling.writable}
        onClick={() => void act('fan', { enabled: !cooling.boosted })}>{cooling.boosted ? '恢复自动' : '一键强冷'}</button></div>
    <p className="linux-muted">隐藏窗口后继续运行，退出程序时恢复原风扇策略。</p>
    {cooling.error && <p className="linux-warning">{cooling.error}</p>}
  </RevCard>;
}

export function CpuPowerControls({ domains, disabled, act }: DomainControlProps) {
  const power = domains.performance;
  const [draft, setDraft] = useState<CpuLimits | null>(power.limits);
  const [dirty, setDirty] = useState(false);
  useEffect(() => { if (!dirty) setDraft(power.limits); }, [power.limits?.pl1, power.limits?.pl2, power.limits?.pl4, dirty]);
  const ceiling = power.ceiling;
  const locked = disabled || !power.limits_writable;
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
        onClick={async () => { if (await act('cpu_limits', { limits: draft })) setDirty(false); }}>应用 CPU 功耗</button>
        <button className="toggle-btn" disabled={disabled || !dirty} onClick={() => { setDraft(power.limits); setDirty(false); }}>还原输入</button></div>
      {dirty && !valid && <p className="linux-warning">要求 PL1 ≤ PL2、PL4 ≥ PL2 + 20 W，且不超过出厂上限。</p>}
    </section>}
    <p className="linux-muted">{power.active === 'custom' ? '当前自定义功耗读回' : '当前机身模式的出厂功耗表'}：{power.limits ? `${power.limits.pl1} / ${power.limits.pl2} / ${power.limits.pl4} W` : '未提供'}。</p>
    <p className="linux-muted">保留原版 PL4 瞬时余量规则，三个限值一起校验并应用。功耗表读回不代表 CPU 实际耗电。</p>
    {domains.cooling.curve.active ? <p className="linux-muted">请先恢复原风扇策略，再调整 CPU 功耗。</p> : locked && <p className="linux-muted">连接原装电源并启用 EC 写入后可调节。</p>}
  </RevCard>;
}
