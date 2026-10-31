import { useState } from 'react';
import { FanCurveEditor } from '../tuning/FanCurveEditor';
import { DEFAULT_8_POINT_CURVE } from '../tuning/fanCurveDefaults';
import { LinuxFanCurveConfig } from '../../types/linux';
import { OemControlProps } from './OemHardwareControls';

const draftKey = 'openrevo_linux_fan_curve_v1';
const defaults = (): LinuxFanCurveConfig => ({
  cpu: DEFAULT_8_POINT_CURVE.map(p => ({ ...p })), gpu: DEFAULT_8_POINT_CURVE.map(p => ({ ...p })),
  isolated: true, ramp_enabled: true, ramp_ms: 300,
});

export function validCurveConfig(config: LinuxFanCurveConfig): boolean {
  return typeof config.isolated === 'boolean' && typeof config.ramp_enabled === 'boolean' && config.isolated && [100, 300, 500, 1000].includes(config.ramp_ms)
    && [config.cpu, config.gpu].every(curve => Array.isArray(curve) && curve.length === 8
      && curve.every((p, i) => p && Number.isInteger(p.temp) && Number.isInteger(p.duty)
        && p.temp >= 30 && p.temp <= 95 && p.duty >= 22 && p.duty <= 100
        && (i === 0 || (p.temp > curve[i - 1].temp && p.duty >= curve[i - 1].duty)))
      && curve[0].temp <= 40 && curve[7].temp === 95 && curve[7].duty === 100);
}

function loadDraft(): LinuxFanCurveConfig {
  try { const draft = JSON.parse(localStorage.getItem(draftKey) ?? 'null'); if (draft && validCurveConfig(draft)) return draft; }
  catch { /* A broken local draft never becomes a hardware command. */ }
  return defaults();
}

export function OemFanCurve({ oem, disabled, acOnline, act }: OemControlProps) {
  const [draft, setDraft] = useState(loadDraft);
  const [tab, setTab] = useState<'cpu' | 'gpu'>('cpu');
  const [linked, setLinked] = useState(false);
  const [notice, setNotice] = useState('');
  const curve = oem.fan_curve;
  const valid = validCurveConfig(draft);
  const canApply = !disabled && oem.available && oem.writable && curve?.supported && !curve.fault
    && acOnline && oem.dc_jack && !oem.fan_boost && valid;
  const update = (patch: Partial<LinuxFanCurveConfig>) => { setDraft(previous => ({ ...previous, ...patch })); setNotice('草稿已修改，尚未应用'); };
  return <section className="linux-fan-curve-panel" aria-label="自定义风扇曲线">
    <FanCurveEditor fanCurveTab={tab} onTabChange={setTab} cpuCurve={draft.cpu} gpuCurve={draft.gpu}
      disabled={disabled} minDuty={22} fanIsolated={draft.isolated} onToggleFanIsolated={isolated => update({ isolated })}
      fanRampRate={draft.ramp_enabled} onToggleFanRampRate={ramp_enabled => update({ ramp_enabled })}
      fanRampSpeedMs={draft.ramp_ms} onChangeFanRampSpeed={ramp_ms => update({ ramp_ms })}
      fanCurveLinked={linked} onToggleFanCurveLinked={value => { setLinked(value); if (value) update({ cpu: draft[tab], gpu: draft[tab] }); }}
      onCurveChange={(type, points) => update(linked ? { cpu: points, gpu: points } : { [type]: points })} />
    <div className="sys-sub-panel linux-stack"><div className="linux-control-actions">
      <button className="toggle-btn" disabled={disabled || !valid} onClick={() => {
        try { localStorage.setItem(draftKey, JSON.stringify(draft)); setNotice('草稿已保存；启动时不会自动应用'); }
        catch { setNotice('草稿保存失败，请检查本地存储空间'); }
      }}>保存曲线草稿</button>
      <button className="toggle-btn active-indigo" disabled={!canApply} onClick={async () => {
        setNotice('正在应用曲线…');
        setNotice(await act('set_linux_oem_fan_curve', { config: draft }) ? '上次应用已完成并读回确认' : '应用失败，请检查错误信息和当前状态');
      }}>应用风扇曲线</button>
      <button className="toggle-btn" disabled={disabled || !curve?.active} onClick={async () => {
        if (await act('restore_linux_oem_fan_curve', {})) setNotice('已恢复自定义曲线启用前的风扇策略');
      }}>恢复原风扇策略</button>
      <button className="toggle-btn" disabled={disabled} onClick={() => { setDraft(defaults()); setNotice('已载入原版默认曲线草稿，尚未应用'); }}>原版默认曲线</button>
      {curve?.applied && <button className="toggle-btn" disabled={disabled} onClick={() => { setDraft(structuredClone(curve.applied!)); setNotice('已载入正在使用的曲线'); }}>载入当前曲线</button>}
    </div>
    <p className="linux-muted">{curve?.active ? '当前自定义曲线运行中' : '当前使用原风扇策略；编辑草稿不会改变风扇'}</p>
    {notice && <p className="linux-muted" role="status">{notice}</p>}
    {!valid && <p className="linux-warning">请开启独立输出，保持 8 个节点温度递增、转速不下降，且 95°C 达到 100%。</p>}
    {!curve?.supported && <p className="linux-muted">需要新版 openrevo_ec 驱动才能应用曲线，当前可编辑并保存草稿。</p>}
    {(!acOnline || !oem.dc_jack) && <p className="linux-muted">连接原装电源后可应用；接电实测尚待完成。</p>}
    {oem.fan_boost && <p className="linux-muted">请先关闭一键强冷，再应用曲线。</p>}
    {curve?.fault && <p className="linux-warning" role="alert">曲线恢复失败，驱动正在请求强冷并重试恢复，请检查实际转速。</p>}
    <p className="linux-muted">沿用原版 8 点插值和温度迟滞。退出、失联、断开原装电源或休眠时恢复原策略；重新启动需手动应用。</p>
    </div>
  </section>;
}
