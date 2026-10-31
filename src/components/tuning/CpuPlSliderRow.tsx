import { CSSProperties, useId } from 'react';

export interface CpuPlSliderRowProps {
  label: string; value: number; onChange: (val: number) => void; min: number; max: number;
  step?: number; unit?: string; minMax?: number; maxMax?: number; onAdjustMax?: (delta: number) => void;
  adjustStep?: number; colorVar?: string; tooltip?: string; hint?: string; disabled?: boolean;
}

export function CpuPlSliderRow({ label, value, onChange, min, max, step = 1, unit = 'W', minMax,
  maxMax = 300, onAdjustMax, adjustStep = 5, colorVar = 'var(--color-balance)', tooltip, hint,
  disabled = false }: CpuPlSliderRowProps) {
  const id = useId();
  const changeValue = (n: number) => {
    if (!Number.isFinite(n)) return;
    // Whole-unit controls feed integer IPC fields; fractional steps keep decimals.
    onChange(Number.isInteger(step) ? Math.trunc(n) : n);
  };
  return <div className={`tuning-row shared-slider-row ${disabled ? 'is-disabled' : ''}`}
    style={{ '--slider-accent': colorVar } as CSSProperties}>
    <div className="tuning-row-header">
      <label htmlFor={id} className="tuning-row-label" title={tooltip}>{label}{hint && <small>{hint}</small>}</label>
      <div className="shared-slider-value"><input id={id} type="number" min={min} max={max} step={step}
        value={value} disabled={disabled} className="tuning-input-number"
        onChange={e => changeValue(e.currentTarget.valueAsNumber)} /><span>{unit}</span></div>
    </div>
    <input type="range" aria-label={`${label}滑块`} min={min} max={max} step={step} value={value} disabled={disabled}
      onChange={e => changeValue(e.currentTarget.valueAsNumber)} />
    <div className="shared-slider-bounds"><span>{min}{unit}</span><span>上限: {max}{unit}</span>
      {onAdjustMax && <div className="shared-slider-value">
        <button type="button" onClick={() => onAdjustMax(-adjustStep)} disabled={disabled || max <= (minMax ?? min)}
          title={`以 ${adjustStep}${unit} 步长调小上限`} className="tuning-step-btn">-</button>
        <button type="button" onClick={() => onAdjustMax(adjustStep)} disabled={disabled || max >= maxMax}
          title={`以 ${adjustStep}${unit} 步长调大上限`} className="tuning-step-btn">+</button>
      </div>}
    </div>
  </div>;
}
