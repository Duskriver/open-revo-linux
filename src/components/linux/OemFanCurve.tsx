import { useEffect, useRef, useState } from 'react';
import { FanCurveEditor } from '../tuning/FanCurveEditor';
import { DEFAULT_8_POINT_CURVE } from '../tuning/fanCurveDefaults';
import { LinuxFanCurveConfig } from '../../types/linux';
import { DomainControlProps } from './DomainControls';
import { errorMessage, linuxApi } from '../../lib/linuxApi';

const draftKey = 'openrevo_linux_fan_curve_v1';
const defaults = (): LinuxFanCurveConfig => ({
  cpu: DEFAULT_8_POINT_CURVE.map(p => ({ ...p })), gpu: DEFAULT_8_POINT_CURVE.map(p => ({ ...p })),
  isolated: true, ramp_enabled: true, ramp_ms: 300,
});

export function validCurveConfig(value: unknown): value is LinuxFanCurveConfig {
  if (typeof value !== 'object' || value === null) return false;
  const config = value as Partial<LinuxFanCurveConfig>;
  return config.isolated === true && typeof config.ramp_enabled === 'boolean' && [100, 300, 500, 1000].includes(config.ramp_ms ?? 0)
    && [config.cpu, config.gpu].every(curve => Array.isArray(curve) && curve.length === 8
      && curve.every((p, i) => p && Number.isInteger(p.temp) && Number.isInteger(p.duty)
        && p.temp >= 30 && p.temp <= 95 && p.duty >= 22 && p.duty <= 100
        && (i === 0 || (p.temp > curve[i - 1].temp && p.duty >= curve[i - 1].duty)))
      && curve[0].temp <= 40 && curve[7].temp === 95 && curve[7].duty === 100);
}

interface LoadedDraft { draft: LinuxFanCurveConfig; notice: string }
let pendingLoad: Promise<LoadedDraft> | null = null;

function loadDraft(): Promise<LoadedDraft> {
  // StrictMode and remounts share one in-flight migration.
  if (pendingLoad) return pendingLoad;
  pendingLoad = (async () => {
    const config = await linuxApi.loadConfig();
    if (config) return { draft: config.fan_curve_draft ?? defaults(), notice: '' };
    const legacy = localStorage.getItem(draftKey);
    if (legacy && localStorage.getItem(`${draftKey}_migrated`) !== 'true') {
      let draft: unknown;
      try { draft = JSON.parse(legacy); } catch { /* Keep the original for recovery. */ }
      if (!validCurveConfig(draft)) return { draft: defaults(), notice: '旧草稿无法通过校验，原始数据已保留；当前显示默认草稿' };
      await linuxApi.saveFanCurveDraft(draft);
      // The saved host config is authoritative even if this optional marker fails.
      try { localStorage.setItem(`${draftKey}_migrated`, 'true'); } catch { /* Host copy already saved. */ }
      return { draft, notice: '旧草稿已迁移到应用配置，原副本保留；未应用到硬件' };
    }
    return { draft: defaults(), notice: '' };
  })().finally(() => { pendingLoad = null; });
  return pendingLoad;
}

export function OemFanCurve({ domains, disabled, releaseDisabled, act }: DomainControlProps) {
  const [draft, setDraft] = useState(defaults);
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [loadError, setLoadError] = useState('');
  const [reload, setReload] = useState(0);
  const fileInput = useRef<HTMLInputElement>(null);
  const native = linuxApi.isNative();
  const [tab, setTab] = useState<'cpu' | 'gpu'>('cpu');
  const [linked, setLinked] = useState(false);
  const [notice, setNotice] = useState('');
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setLoadError('');
    if (!native) { setLoading(false); return; }
    void loadDraft().then(result => {
      if (!cancelled) { setDraft(result.draft); setNotice(result.notice); }
    }).catch(error => {
      if (!cancelled) setLoadError(errorMessage(error));
    }).finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [native, reload]);
  const draftBusy = loading || saving;
  const canSave = native && !draftBusy && !loadError;
  const save = async () => {
    setSaving(true);
    try { await linuxApi.saveFanCurveDraft(draft); setNotice('草稿已保存；启动时不会自动应用'); }
    catch (error) { setNotice(`草稿保存失败：${errorMessage(error)}`); }
    finally { setSaving(false); }
  };
  const exportDraft = async () => {
    setSaving(true);
    try {
      const json = await linuxApi.exportConfig();
      if (json === null) { setNotice('请先保存草稿，再导出'); return; }
      const url = URL.createObjectURL(new Blob([json], { type: 'application/json' }));
      const link = document.createElement('a'); link.href = url;
      link.download = 'openrevo-settings-v3.json'; link.click();
      setTimeout(() => URL.revokeObjectURL(url), 1000);
      setNotice('已导出已保存的草稿与模式设置；未保存的编辑不包含在文件内');
    } catch (error) { setNotice(`导出失败：${errorMessage(error)}`); }
    finally { setSaving(false); }
  };
  const importDraft = async (file: File) => {
    setSaving(true);
    try {
      if (file.size > 65536) throw new Error('配置文件不能超过 64 KiB');
      const config = await linuxApi.importConfig(await file.text());
      setDraft(config.fan_curve_draft ?? defaults());
      setNotice('配置已导入，草稿与模式设置已替换；未应用到硬件，原配置保留为备份');
    } catch (error) { setNotice(`导入失败：${errorMessage(error)}`); }
    finally { setSaving(false); }
  };
  const curve = domains.cooling.curve;
  const valid = validCurveConfig(draft);
  const canApply = !disabled && !draftBusy && !loadError && domains.cooling.curve_writable && valid;
  const update = (patch: Partial<LinuxFanCurveConfig>) => { setDraft(previous => ({ ...previous, ...patch })); setNotice('草稿已修改，尚未应用'); };
  return <section className="linux-fan-curve-panel" aria-label="自定义风扇曲线">
    <FanCurveEditor fanCurveTab={tab} onTabChange={setTab} cpuCurve={draft.cpu} gpuCurve={draft.gpu}
      disabled={draftBusy} minDuty={22} fanIsolated={draft.isolated} onToggleFanIsolated={isolated => update({ isolated })}
      fanRampRate={draft.ramp_enabled} onToggleFanRampRate={ramp_enabled => update({ ramp_enabled })}
      fanRampSpeedMs={draft.ramp_ms} onChangeFanRampSpeed={ramp_ms => update({ ramp_ms })}
      fanCurveLinked={linked} onToggleFanCurveLinked={value => { setLinked(value); if (value) update({ cpu: draft[tab], gpu: draft[tab] }); }}
      onCurveChange={(type, points) => update(linked ? { cpu: points, gpu: points } : { [type]: points })} />
    <div className="sys-sub-panel linux-stack"><div className="linux-control-actions">
      <button className="toggle-btn" disabled={!canSave || !valid} onClick={() => void save()}>保存曲线草稿</button>
      <button className="toggle-btn" disabled={!canSave} onClick={() => void exportDraft()}>导出应用配置</button>
      <button className="toggle-btn" disabled={!canSave} onClick={() => fileInput.current?.click()}>导入并替换配置</button>
      <input ref={fileInput} type="file" accept=".json,application/json" hidden aria-label="导入曲线配置文件"
        onChange={event => { const file = event.target.files?.[0]; event.target.value = ''; if (file) void importDraft(file); }} />
      <button className="toggle-btn active-indigo" disabled={!canApply} onClick={async () => {
        setNotice('正在应用曲线…');
        setNotice(await act('fan_curve', { config: draft }) ? '上次应用已完成并读回确认' : '应用失败，请检查错误信息和当前状态');
      }}>应用风扇曲线</button>
      <button className="toggle-btn" disabled={releaseDisabled || !curve?.active} onClick={async () => {
        if (await act('restore_fan_curve', {})) setNotice('已恢复自定义曲线启用前的风扇策略');
      }}>恢复原风扇策略</button>
      <button className="toggle-btn" disabled={draftBusy} onClick={() => { setDraft(defaults()); setNotice('已载入原版默认曲线草稿，尚未应用'); }}>原版默认曲线</button>
      {curve?.applied && <button className="toggle-btn" disabled={draftBusy || disabled} onClick={() => { setDraft(structuredClone(curve.applied!)); setNotice('已载入正在使用的曲线'); }}>载入当前曲线</button>}
    </div>
    <p className="linux-muted">{curve?.active ? '当前自定义曲线运行中' : '当前使用原风扇策略；编辑草稿不会改变风扇'}</p>
    {loading && <p className="linux-muted" role="status">正在读取草稿…</p>}
    <p className="sys-row-desc">配置导入支持 v1 / v2 / v3，会替换已保存的设置；旧场景仅保留为历史数据，不会执行。</p>
    {loadError && <div className="linux-warning" role="alert">配置读取失败：{loadError}。原文件保持不变。
      <button className="toggle-btn" disabled={draftBusy} onClick={() => setReload(value => value + 1)}>重试读取草稿</button></div>}
    {!native && <p className="linux-muted">请在桌面程序中保存和导入草稿。</p>}
    {notice && <p className="linux-muted" role="status">{notice}</p>}
    {!valid && <p className="linux-warning">请开启独立输出，保持 8 个节点温度递增、转速不下降，且 95°C 达到 100%。</p>}
    {!curve?.supported && <p className="linux-muted">需要新版 openrevo_ec 驱动才能应用曲线，当前可编辑并保存草稿。</p>}
    {!domains.cooling.curve_writable && <p className="linux-muted">连接原装电源后可应用；接电实测尚待完成。</p>}
    {domains.cooling.boosted && <p className="linux-muted">请先关闭一键强冷，再应用曲线。</p>}
    {curve?.fault && <p className="linux-warning" role="alert">曲线恢复失败，驱动正在请求强冷并重试恢复，请检查实际转速。</p>}
    <p className="linux-muted">沿用原版 8 点插值和温度迟滞。退出、后台失联、断开原装电源或休眠时恢复原策略；重新启动需手动应用。</p>
    </div>
  </section>;
}
