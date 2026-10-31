import React, { useRef, useState, useEffect, useId } from 'react';
import { Wind, Link2, Link2Off, Plus, Minus, Fan, ChevronDown } from 'lucide-react';
import { FanCurvePoint } from '../../types/hardware';
import { RevSwitch } from '../ui/RevSwitch';

export interface FanCurveEditorProps {
  fanCurveTab: 'cpu' | 'gpu';
  onTabChange: (tab: 'cpu' | 'gpu') => void;
  cpuCurve: FanCurvePoint[];
  gpuCurve: FanCurvePoint[];
  onCurveChange: (type: 'cpu' | 'gpu', newCurve: FanCurvePoint[]) => void;
  fanIsolated: boolean;
  onToggleFanIsolated: (enabled: boolean) => void;
  fanRampRate: boolean;
  onToggleFanRampRate: (enabled: boolean) => void;
  fanRampSpeedMs: number;
  onChangeFanRampSpeed: (speedMs: number) => void;
  fanCurveLinked?: boolean;
  onToggleFanCurveLinked?: (linked: boolean) => void;
  fanBoost?: boolean;
  onToggleFanBoost?: () => void;
  fanBoostMode?: string;
  maxRpm?: number;
  fanIndependentChannels?: boolean;
  supportFanRampRate?: boolean;
  minDuty?: number;
  disabled?: boolean;
}

export const FanCurveEditor: React.FC<FanCurveEditorProps> = ({
  fanCurveTab,
  onTabChange,
  cpuCurve,
  gpuCurve,
  onCurveChange,
  fanIsolated,
  onToggleFanIsolated,
  fanRampRate,
  onToggleFanRampRate,
  fanRampSpeedMs,
  onChangeFanRampSpeed,
  fanCurveLinked = false,
  onToggleFanCurveLinked,
  fanBoost = false,
  onToggleFanBoost,
  fanBoostMode,
  maxRpm = 5800,
  fanIndependentChannels = true,
  supportFanRampRate = true,
  minDuty: propMinDuty,
  disabled = false,
}) => {
  const gradientId = `fan-curve-${useId().replace(/:/g, '')}`;
  const minDuty = propMinDuty ?? (fanBoostMode === 'direct_rpm_clamp' ? 22 : 0);
  const [draggingIdx, setDraggingIdx] = useState<number | null>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const repeatTimerRef = useRef<{ timeout?: any; interval?: any }>({});

  // Fan curve chart dimensions (expanded by ~50% vertically for generous, precise dragging)
  const SVG_W = 320;
  const SVG_H = 285;
  const PAD_L = 32;
  const PAD_R = 14;
  const PAD_T = 20;
  const PAD_B = 26;
  const PLOT_W = SVG_W - PAD_L - PAD_R; // 274
  const PLOT_H = SVG_H - PAD_T - PAD_B; // 239

  const getSvgX = (temp: number) => {
    const clamped = Math.max(30, Math.min(100, temp));
    return PAD_L + ((clamped - 30) / 70) * PLOT_W;
  };

  const getSvgY = (duty: number) => {
    const clamped = Math.max(0, Math.min(100, duty));
    return PAD_T + (1 - clamped / 100) * PLOT_H;
  };

  // Fan curve duty change with strict Monotonic Non-Decreasing constraint
  const handleCurveDutyChange = (type: 'cpu' | 'gpu', index: number, duty: number) => {
    const baseCurve = type === 'cpu' ? cpuCurve : gpuCurve;
    const newCurve = [...baseCurve];
    if (newCurve[index]) {
      const targetDuty = Math.max(minDuty, Math.min(100, Math.round(duty)));
      newCurve[index] = { ...newCurve[index], duty: targetDuty };

      // Monotonic Rule 1: Points after current index cannot have lower duty (push higher)
      for (let j = index + 1; j < newCurve.length; j++) {
        if (newCurve[j].duty < targetDuty) {
          newCurve[j] = { ...newCurve[j], duty: targetDuty };
        }
      }

      // Monotonic Rule 2: Points before current index cannot have higher duty (pull lower), but floor is minDuty
      for (let j = 0; j < index; j++) {
        if (newCurve[j].duty > targetDuty) {
          newCurve[j] = { ...newCurve[j], duty: Math.max(minDuty, targetDuty) };
        }
      }

      onCurveChange(type, newCurve);
    }
  };

  // Fine-tuning 1% step handler with continuous long-press repeat
  const stopRepeat = () => {
    if (repeatTimerRef.current.timeout) clearTimeout(repeatTimerRef.current.timeout);
    if (repeatTimerRef.current.interval) clearInterval(repeatTimerRef.current.interval);
    repeatTimerRef.current = {};
  };

  const isEditingAllowed = !disabled && (fanIndependentChannels ? fanIsolated : true);

  const handleStepDuty = (index: number, delta: number) => {
    if (!isEditingAllowed) return;
    const baseCurve = fanCurveTab === 'cpu' ? cpuCurve : gpuCurve;
    const curVal = baseCurve[index]?.duty ?? minDuty;
    const nextVal = Math.max(minDuty, Math.min(100, curVal + delta));
    handleCurveDutyChange(fanCurveTab, index, nextVal);
    if (!fanIndependentChannels) {
      handleCurveDutyChange(fanCurveTab === 'cpu' ? 'gpu' : 'cpu', index, nextVal);
    }
  };

  const stepRef = useRef(handleStepDuty);
  stepRef.current = handleStepDuty;

  const startRepeat = (index: number, delta: number) => {
    if (!isEditingAllowed) return;
    stopRepeat();
    handleStepDuty(index, delta);
    repeatTimerRef.current.timeout = setTimeout(() => {
      repeatTimerRef.current.interval = setInterval(() => {
        stepRef.current(index, delta);
      }, 50);
    }, 260);
  };

  useEffect(() => {
    window.addEventListener('pointerup', stopRepeat);
    window.addEventListener('pointercancel', stopRepeat);
    window.addEventListener('blur', stopRepeat);
    return () => { stopRepeat(); window.removeEventListener('pointerup', stopRepeat); window.removeEventListener('pointercancel', stopRepeat); window.removeEventListener('blur', stopRepeat); };
  }, []);

  const handleSvgPointerMove = (e: React.PointerEvent<SVGSVGElement>) => {
    if (draggingIdx === null || !isEditingAllowed || !svgRef.current) return;
    const rect = svgRef.current.getBoundingClientRect();
    const clientY = e.clientY - rect.top;
    const scaleY = SVG_H / rect.height;
    const svgY = clientY * scaleY;
    const rawDuty = Math.round(100 * (1 - (svgY - PAD_T) / PLOT_H));
    // Precise 1% continuous resolution, strictly bounded in [minDuty, 100]
    const clampedDuty = Math.max(minDuty, Math.min(100, rawDuty));
    handleCurveDutyChange(fanCurveTab, draggingIdx, clampedDuty);
    if (!fanIndependentChannels) {
      handleCurveDutyChange(fanCurveTab === 'cpu' ? 'gpu' : 'cpu', draggingIdx, clampedDuty);
    }
  };

  const pts = fanCurveTab === 'cpu' ? cpuCurve : gpuCurve;
  const mappedPts = pts.map((p) => ({
    x: getSvgX(p.temp),
    y: getSvgY(p.duty),
    temp: p.temp,
    duty: p.duty,
  }));

  const startX = PAD_L;
  const endX = PAD_L + PLOT_W;
  const floorY = PAD_T + PLOT_H;
  const startY = mappedPts.length > 0 ? mappedPts[0].y : getSvgY(minDuty > 0 ? minDuty : 20);
  const endY = mappedPts.length > 0 ? mappedPts[mappedPts.length - 1].y : getSvgY(100);

  const pointsStr = mappedPts.map((p) => `${p.x},${p.y}`).join(' L ');
  const linePath = mappedPts.length > 0
    ? `M ${startX},${startY} L ${pointsStr} L ${endX},${endY}`
    : '';
  const areaPath = mappedPts.length > 0
    ? `M ${startX},${floorY} L ${startX},${startY} L ${pointsStr} L ${endX},${endY} L ${endX},${floorY} Z`
    : '';

  return <div className={`tuning-card shared-fan-editor ${isEditingAllowed ? 'editing' : ''}`}>
    <div className="tuning-header"><div className="tuning-header-title"><Wind size={16} /><span>风扇策略与曲线</span></div></div>
    {fanBoostMode === 'direct_rpm_clamp' && onToggleFanBoost && <div className="quick-row">
      <span className="sys-row-label"><Fan size={14} />硬件物理强锁 ({maxRpm} RPM)</span>
      <RevSwitch checked={fanBoost} disabled={disabled} onChange={() => onToggleFanBoost()} size="sm" />
    </div>}
    {supportFanRampRate && <div className="quick-row"><span className="sys-row-label">平滑过渡</span>
      <div className="sys-row-controls"><span className="shared-fan-select"><select aria-label="风扇平滑过渡时间" className="rev-select" disabled={disabled || !fanRampRate}
        value={fanRampSpeedMs} onChange={e => onChangeFanRampSpeed(Number(e.target.value))}>
        {[100, 300, 500, 1000].map(ms => <option key={ms} value={ms}>{ms}ms</option>)}</select><ChevronDown size={12} aria-hidden="true" /></span>
        <RevSwitch checked={fanRampRate} disabled={disabled} onChange={onToggleFanRampRate} size="sm" /></div>
    </div>}
    {fanIndependentChannels && <div className="quick-row"><span className="sys-row-label">风扇独立输出</span>
      <RevSwitch checked={fanIsolated} disabled={disabled} onChange={onToggleFanIsolated} size="sm" /></div>}
    <div className="shared-fan-body"><div className="shared-fan-toolbar">
      {fanIndependentChannels ? <div className="shared-fan-tabs">
        <button type="button" className={`toggle-btn ${fanCurveTab === 'cpu' ? 'active-indigo' : ''}`} disabled={disabled} onClick={() => onTabChange('cpu')}>CPU风扇</button>
        {onToggleFanCurveLinked && <button type="button" className={`toggle-btn ${fanCurveLinked ? 'active-indigo' : ''}`} disabled={disabled}
          aria-label="联动 CPU 和 GPU 曲线" aria-pressed={fanCurveLinked} title="双风扇曲线联动"
          onClick={() => onToggleFanCurveLinked(!fanCurveLinked)}>{fanCurveLinked ? <Link2 size={14} /> : <Link2Off size={14} />}</button>}
        <button type="button" className={`toggle-btn ${fanCurveTab === 'gpu' ? 'active-indigo' : ''}`} disabled={disabled} onClick={() => onTabChange('gpu')}>GPU风扇</button>
      </div> : <span className="shared-fan-tabs"><Link2 size={14} />双风扇联动</span>}
      <span className="shared-fan-state">{isEditingAllowed ? '● 8点曲线调校' : '○ 出厂预设托管'}</span>
    </div>
    <div className="shared-fan-plot"><svg ref={svgRef} viewBox={`0 0 ${SVG_W} ${SVG_H}`} aria-label={`${fanCurveTab.toUpperCase()} 风扇曲线`}
      onPointerMove={handleSvgPointerMove} onPointerUp={() => setDraggingIdx(null)} onPointerCancel={() => setDraggingIdx(null)}>
      <defs><linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1"><stop offset="0%" stopColor="var(--fan-curve-accent)" stopOpacity="0.4" />
        <stop offset="100%" stopColor="var(--fan-curve-accent)" stopOpacity="0.02" /></linearGradient></defs>
      {[0, 25, 50, 75, 100].map(duty => <g key={duty}><line className="shared-fan-grid" x1={PAD_L} y1={getSvgY(duty)} x2={endX} y2={getSvgY(duty)} strokeDasharray="3 3" />
        <text className="shared-fan-axis" x={PAD_L - 4} y={getSvgY(duty) + 3} textAnchor="end">{duty}%</text></g>)}
      {mappedPts.map((pt, idx) => <g key={`guide-${idx}`}><line className="shared-fan-grid" x1={pt.x} y1={PAD_T} x2={pt.x} y2={floorY} strokeDasharray="2 3" />
        <text className="shared-fan-axis" x={pt.x} y={floorY + 14} textAnchor="middle">{pt.temp}°</text></g>)}
      <path d={areaPath} fill={`url(#${gradientId})`} /><path className="shared-fan-line" d={linePath} fill="none" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" />
      {mappedPts.map((pt, idx) => <g key={idx} className={`shared-fan-node ${draggingIdx === idx ? 'dragging' : ''}`}
        role="slider" tabIndex={isEditingAllowed ? 0 : -1} aria-label={`${fanCurveTab.toUpperCase()} ${pt.temp}°C 转速`}
        aria-valuemin={minDuty} aria-valuemax={100} aria-valuenow={pt.duty} aria-disabled={!isEditingAllowed}
        onPointerDown={e => { if (isEditingAllowed) { e.currentTarget.setPointerCapture(e.pointerId); setDraggingIdx(idx); } }}
        onKeyDown={e => { if (['ArrowUp', 'ArrowRight', 'ArrowDown', 'ArrowLeft'].includes(e.key)) { e.preventDefault(); handleStepDuty(idx, ['ArrowUp', 'ArrowRight'].includes(e.key) ? 1 : -1); } }}
        onWheel={e => { if (isEditingAllowed) { e.preventDefault(); handleStepDuty(idx, e.deltaY < 0 ? 1 : -1); } }}>
        <circle cx={pt.x} cy={pt.y} r={18} fill="transparent" /><circle className="shared-fan-node-ring" cx={pt.x} cy={pt.y} r={draggingIdx === idx ? 12 : 7.5} />
        <circle className="shared-fan-node-core" cx={pt.x} cy={pt.y} r={draggingIdx === idx ? 6.5 : 4.5} />
        <text className="shared-fan-node-label" x={pt.x} y={pt.y - (draggingIdx === idx ? 14 : 10)} textAnchor="middle">{pt.duty}%</text>
      </g>)}
    </svg></div>
    <div className="shared-fan-points">{pts.map((pt, idx) => <div key={idx} className="shared-fan-point">
      <button type="button" className="toggle-btn" aria-label={`${fanCurveTab.toUpperCase()} ${pt.temp}°C 增加转速`}
        disabled={!isEditingAllowed || pt.duty >= 100} onPointerDown={e => { e.preventDefault(); startRepeat(idx, 1); }}
        onClick={e => { if (e.detail === 0) handleStepDuty(idx, 1); }} onPointerUp={stopRepeat} onPointerCancel={stopRepeat} onPointerLeave={stopRepeat} onBlur={stopRepeat}><Plus size={11} /></button>
      <strong>{pt.duty}%</strong>
      <button type="button" className="toggle-btn" aria-label={`${fanCurveTab.toUpperCase()} ${pt.temp}°C 减少转速`}
        disabled={!isEditingAllowed || pt.duty <= minDuty} onPointerDown={e => { e.preventDefault(); startRepeat(idx, -1); }}
        onClick={e => { if (e.detail === 0) handleStepDuty(idx, -1); }} onPointerUp={stopRepeat} onPointerCancel={stopRepeat} onPointerLeave={stopRepeat} onBlur={stopRepeat}><Minus size={11} /></button>
      <span>{pt.temp}°</span></div>)}</div>
    <div className="shared-fan-help"><span>{minDuty > 0 ? `最低 ${minDuty}% · 滚轮 / 方向键微调` : '滚轮 / 方向键微调'}</span><span>8 节点单调递增</span></div>
    </div>
  </div>;
};
