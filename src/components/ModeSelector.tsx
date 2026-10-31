import { POWER_MODES, PowerMode } from '../types/hardware';
import { ModelCapabilities } from '../plugins/plugin_manifest';
import { Shield, Zap, Flame, Sliders, Settings2 } from 'lucide-react';

interface ModeSelectorProps {
  currentMode: PowerMode | number;
  onSelectMode: (mode: PowerMode) => void;
  disabled?: boolean;
  isDcOnline?: boolean;
  unlockBatteryLimit?: boolean;
  onOpenCustomTuning?: () => void;
  capabilities?: ModelCapabilities;
  isThinLight?: boolean;
  unavailableModes?: Partial<Record<PowerMode, string>>;
  modeLabels?: Partial<Record<PowerMode, string>>;
}

export function ModeSelector({ currentMode, onSelectMode, disabled = false, isDcOnline = true,
  unlockBatteryLimit = false, onOpenCustomTuning, capabilities, isThinLight = false,
  unavailableModes = {}, modeLabels = {} }: ModeSelectorProps) {
  const isLight = isThinLight || capabilities?.form_factor === 'thin_light';
  const hasCustomMode = capabilities?.is_adapted !== false &&
    (capabilities?.power_modes ? capabilities.power_modes.includes('custom') : !isLight);
  const modes = POWER_MODES.filter(m => hasCustomMode || m.id !== 4);
  const icons = { 1: Shield, 2: Zap, 3: Flame, 4: Sliders };
  const variants = { 1: 'office', 2: 'balance', 3: 'beast', 4: 'custom' };
  return <div className={`shared-mode-grid ${modes.length === 3 ? 'three-modes' : ''}`}>
    {modes.map(mode => {
      const active = currentMode === mode.id;
      const custom = mode.id === 4;
      const restricted = !isLight && (mode.id === 3 || custom) && !isDcOnline && !unlockBatteryLimit;
      const reason = unavailableModes[mode.id] ?? (restricted ? '电池供电或受限供电下不可用，需接入原装电源适配器' : undefined);
      const Icon = icons[mode.id];
      return <button key={mode.id} className={`mode-btn shared-mode-btn mode-${variants[mode.id]} ${active ? `active-${variants[mode.id]}` : ''}`}
        disabled={disabled || !!reason} aria-pressed={active}
        title={reason ?? (custom && active ? '打开自定义硬件参数调校' : `${mode.nameEn} - ${mode.desc}`)}
        onClick={() => { if (active && custom && onOpenCustomTuning) onOpenCustomTuning(); else onSelectMode(mode.id); }}>
        {custom && (active || isLight) && onOpenCustomTuning && !reason && <span className="shared-mode-tuning"
          onClick={e => { if (disabled) return; e.stopPropagation(); onOpenCustomTuning(); }}><Settings2 size={10} />调校</span>}
        <Icon size={16} />
        <span className="mode-title">{modeLabels[mode.id] ?? mode.name.replace('模式', '')}</span>
        <span className="mode-sub">{reason ? '不可用' : active ? '● 当前' : custom ? '极客' : mode.id === 1 ? '静音' : mode.id === 2 ? '日常' : '狂暴'}</span>
      </button>;
    })}
  </div>;
}
