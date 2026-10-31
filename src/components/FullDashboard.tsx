import React, { useState, useEffect, useRef, useCallback } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { HardwareState, PowerMode } from '../types/hardware';
import { OverviewView } from './OverviewView';
import { LightingStudio } from './LightingStudio';
import { GpuModeView } from './GpuModeView';
import { PowerTuningView } from './PowerTuningView';
import { SponsorView } from './SponsorView';
import { CustomTuningPanel } from './CustomTuningPanel';
import { WaterCoolerView, CoolerTelemetry } from './WaterCoolerView';
import { SystemSettingsView, OsdConfig } from './SystemSettingsView';
import { PluginsView, CustomPluginItem } from './PluginsView';
import { PluginTabHost } from './PluginTabHost';
import { ModuleErrorBoundary } from './ModuleErrorBoundary';
import { BUILTIN_MODULE_SLOTS, BuiltinTabId, isSlotActive, ModelCapabilities } from '../plugins/plugin_manifest';
import { DashboardChrome } from './common/DashboardChrome';
import {
  Minimize2,
  X,
  Minus,
  Shield,
  Loader2,
  Zap,
  Square,
  Copy,
  Layers,
} from 'lucide-react';

export type TabId = BuiltinTabId | string;

interface FullDashboardProps {
  status: HardwareState;
  theme?: 'mono' | 'cyber';
  activeSkin?: string | null;
  activeBackground?: string | null;
  onApplySkin?: (skinId: string | null) => void;
  onApplyBackground?: (backgroundId: string | null) => void;
  onToggleTheme?: () => void;
  onSetPowerMode: (mode: PowerMode) => void;
  onToggleFanBoost: (targetRpm?: number) => void;
  onToggleBatteryLimit: () => void;
  onSwitchRefreshRate: (hz: number, invokeBackend?: boolean) => void;
  onToggleWinKey: () => void;
  onToggleAutostart: () => void;
  onToggleOemService?: (takeOver: boolean) => Promise<void> | void;
  onCollapseDashboard: () => void;
  onMinimize: () => void;
  onClose: () => void;
  initialTab?: TabId;
  initialOpenCustomTuning?: boolean;
  capabilities?: ModelCapabilities;
  previewMode?: 'real' | 'thin_light' | 'gaming';
  onPreviewFormFactor?: (mode: 'real' | 'thin_light' | 'gaming') => void;
}

import { invoke } from '@tauri-apps/api/core';

export const FullDashboard: React.FC<FullDashboardProps> = ({
  status,
  theme = 'mono',
  activeSkin,
  activeBackground,
  onApplySkin,
  onApplyBackground,
  onToggleTheme,
  onSetPowerMode,
  onToggleFanBoost,
  onToggleBatteryLimit,
  onSwitchRefreshRate,
  onToggleWinKey,
  onToggleAutostart,
  onToggleOemService,
  onCollapseDashboard,
  onMinimize,
  onClose,
  initialTab,
  initialOpenCustomTuning = false,
  capabilities: propCapabilities,
  previewMode: propPreviewMode,
  onPreviewFormFactor: propOnPreviewFormFactor,
}) => {
  const [activeTab, setActiveTab] = useState<TabId>(initialTab || 'overview');
  const [isCustomTuningOpen, setIsCustomTuningOpen] = useState(initialOpenCustomTuning);
  const [showOemModal, setShowOemModal] = useState(false);
  const [isSwitchingService, setIsSwitchingService] = useState(false);
  const [profileVersion, setProfileVersion] = useState(0);
  const checkOemGuard = (action: () => void) => {
    if (status.oem_running) {
      setShowOemModal(true);
      return;
    }
    action();
  };

  const handleTakeoverOem = async () => {
    setIsSwitchingService(true);
    try {
      if (onToggleOemService) {
        await onToggleOemService(true);
      } else {
        await invoke('toggle_oem_service', { takeOver: true });
      }
      setShowOemModal(false);
    } finally {
      setIsSwitchingService(false);
    }
  };

  useEffect(() => {
    if (initialTab) {
      setActiveTab(initialTab);
    }
  }, [initialTab]);

  useEffect(() => {
    if (initialOpenCustomTuning) {
      setIsCustomTuningOpen(true);
    }
  }, [initialOpenCustomTuning]);

  // Persistent Module Toggles (Safe-Mode Isolation)
  const [moduleToggles, setModuleToggles] = useState<Record<string, boolean>>({});

  useEffect(() => {
    invoke<Record<string, boolean>>('get_module_toggles')
      .then((res) => {
        if (res) setModuleToggles(res);
      })
      .catch((err) => console.warn('Failed to load module toggles:', err));
  }, []);

  const [localWaterCoolerEnabled, setLocalWaterCoolerEnabled] = useState<boolean | null>(null);
  const isWaterCoolerEnabled = localWaterCoolerEnabled !== null
    ? localWaterCoolerEnabled
    : (moduleToggles.water !== undefined
        ? moduleToggles.water
        : (status.water_cooler_enabled ?? status.support_flags?.has_water_cooler ?? false));

  useEffect(() => {
    if (typeof moduleToggles.water === 'boolean') {
      setLocalWaterCoolerEnabled(moduleToggles.water);
    } else if (typeof status.water_cooler_enabled === 'boolean') {
      setLocalWaterCoolerEnabled(status.water_cooler_enabled);
    }
  }, [moduleToggles.water, status.water_cooler_enabled]);

  // Safeguard: if water cooler module is disabled, fallback to overview
  useEffect(() => {
    if (!isWaterCoolerEnabled && activeTab === 'water') {
      setActiveTab('overview');
    }
  }, [isWaterCoolerEnabled, activeTab]);

  // Model Capabilities (Declarative Hardware Driver Contract)
  const realCapabilitiesRef = useRef<ModelCapabilities | undefined>(undefined);
  const [internalCapabilities, setInternalCapabilities] = useState<ModelCapabilities | undefined>(undefined);
  const [internalPreviewMode, setInternalPreviewMode] = useState<'real' | 'thin_light' | 'gaming'>('real');

  const capabilities = propCapabilities !== undefined ? propCapabilities : internalCapabilities;
  const previewMode = propPreviewMode !== undefined ? propPreviewMode : internalPreviewMode;

  useEffect(() => {
    if (propCapabilities === undefined) {
      invoke<ModelCapabilities>('get_model_capabilities')
        .then((caps) => {
          if (caps) {
            realCapabilitiesRef.current = caps;
            setInternalCapabilities(caps);
          }
        })
        .catch((err) => console.warn('Failed to load model capabilities:', err));
    }
  }, [propCapabilities]);

  const handlePreviewFormFactor = useCallback((mode: 'real' | 'thin_light' | 'gaming') => {
    if (propOnPreviewFormFactor) {
      propOnPreviewFormFactor(mode);
      return;
    }
    setInternalPreviewMode(mode);
    if (mode === 'real') {
      if (realCapabilitiesRef.current) {
        setInternalCapabilities({ ...realCapabilitiesRef.current });
      }
    } else if (mode === 'thin_light') {
      setInternalCapabilities({
        ...(realCapabilitiesRef.current || {}),
        id: 'preview_xingyao_14',
        name: 'MECHREVO Xingyao 14 (UI Preview)',
        form_factor: 'thin_light',
        ui_preset: 'ultrabook_compact',
        discrete_gpu: false,
        water_cooling: false,
        fan_custom_curve: false,
        battery_charge_limit: true,
        power_modes: ['office', 'balance', 'turbo'],
        rgb_keyboard: { supported: true, type: 'monochrome_stepped', steps: [0, 1, 2, 3, 4] },
        is_debug_mode: true,
      });
    } else if (mode === 'gaming') {
      setInternalCapabilities({
        ...(realCapabilitiesRef.current || {}),
        id: 'preview_gaming',
        name: 'MECHREVO Gaming Laptop (UI Preview)',
        form_factor: 'gaming',
        ui_preset: 'gaming_full',
        discrete_gpu: true,
        water_cooling: true,
        fan_custom_curve: true,
        battery_charge_limit: true,
        power_modes: ['office', 'balance', 'turbo', 'custom'],
        rgb_keyboard: { supported: true, type: 'per_key_rgb' },
        is_debug_mode: true,
      });
    }
  }, [propOnPreviewFormFactor]);

  // On non-dGPU machines, GPU tab remains accessible for display brightness & color tuning controls

  const handleToggleModule = async (id: string, enabled: boolean) => {
    setModuleToggles((prev) => ({ ...prev, [id]: enabled }));
    try {
      await invoke('set_module_enabled', { id, enabled });
      if (id === 'water') {
        setLocalWaterCoolerEnabled(enabled);
      }
    } catch (e) {
      console.error('Failed to update module toggle:', e);
      setModuleToggles((prev) => ({ ...prev, [id]: !enabled }));
    }
  };

  // Safe Mode fallback: if currently viewing a tab that got disabled, safely revert to overview
  useEffect(() => {
    if (moduleToggles[activeTab] === false) {
      setActiveTab('overview');
    }
  }, [moduleToggles, activeTab]);

  // Third-party Tab Plugins State
  const [tabPlugins, setTabPlugins] = useState<CustomPluginItem[]>([]);

  const loadTabPlugins = useCallback(async () => {
    try {
      const plugins = await invoke<CustomPluginItem[]>('get_custom_plugins');
      if (plugins) {
        setTabPlugins(plugins.filter((p) => p.plugin_type === 'tab' && p.enabled && p.is_compatible !== false));
      }
      const toggles = await invoke<Record<string, boolean>>('get_module_toggles');
      if (toggles) {
        setModuleToggles(toggles);
      }
    } catch (e) {
      console.warn('Failed to load tab plugins or module toggles:', e);
    }
  }, []);

  useEffect(() => {
    loadTabPlugins();
  }, [loadTabPlugins]);

  // Safe fallback if custom tab got disabled
  useEffect(() => {
    const isBuiltin = BUILTIN_MODULE_SLOTS.some((s) => s.id === activeTab);
    const isCustomActive = tabPlugins.some((p) => p.id === activeTab);
    if (!isBuiltin && !isCustomActive && activeTab !== 'overview') {
      setActiveTab('overview');
    }
  }, [tabPlugins, activeTab]);

  const [isCoolerConnected, setIsCoolerConnected] = useState(false);

  useEffect(() => {
    invoke<CoolerTelemetry>('get_water_cooler_status')
      .then((res) => {
        if (res) setIsCoolerConnected(res.connected);
      })
      .catch(() => {});
  }, []);

  const [osdConfig, setOsdConfig] = useState<OsdConfig>({
    enabled: true,
    theme: 'cinema',
    scale: '100%',
    position: 'bottom-center',
  });

  useEffect(() => {
    invoke<any>('get_osd_config')
      .then((res) => {
        if (res) {
          setOsdConfig({
            enabled: res.enabled ?? true,
            theme: res.theme || 'cinema',
            scale: res.scale || '100%',
            position: res.position || 'bottom-center',
          });
        }
      })
      .catch(() => {});
  }, []);

  const handleUpdateOsdConfig = async (newCfg: OsdConfig) => {
    setOsdConfig(newCfg);
    try {
      await invoke('save_osd_config', {
        config: newCfg,
        enabled: newCfg.enabled,
        theme: newCfg.theme,
        scale: newCfg.scale,
        position: newCfg.position,
      });
    } catch (err) {
      console.error('Failed to save OSD config:', err);
    }
  };

  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    try {
      const win = getCurrentWindow();
      win.isMaximized().then(setIsMaximized).catch(() => {});
      let unlistenResize: (() => void) | undefined;
      win.listen('tauri://resize', async () => {
        try {
          const max = await win.isMaximized();
          setIsMaximized(max);
        } catch (_) {}
      }).then((fn) => { unlistenResize = fn; });

      return () => {
        if (unlistenResize) unlistenResize();
      };
    } catch (_) {}
  }, []);

  const handleToggleMaximize = async () => {
    try {
      const res = await invoke<boolean>('toggle_maximize_window');
      setIsMaximized(res);
    } catch (e) {
      console.warn('Failed to toggle maximize:', e);
      try {
        const win = getCurrentWindow();
        const max = await win.isMaximized();
        if (max) {
          await win.unmaximize();
          await invoke('resize_window', { expanded: true });
          setIsMaximized(false);
        } else {
          await win.maximize();
          setIsMaximized(true);
        }
      } catch (_) {}
    }
  };


  return (

    <div
      className="acrylic-container openrevo-shell"
      data-slot="shell"
      data-theme={theme}
      data-form-factor={capabilities?.form_factor || 'gaming'}
    >
      <DashboardChrome
        theme={theme} onToggleTheme={onToggleTheme} activeTab={activeTab} onSelectTab={setActiveTab}
        mock={status.is_mock} onToggleMaximize={handleToggleMaximize}
        tabs={[
          ...BUILTIN_MODULE_SLOTS.filter(slot => isSlotActive(slot, { status, isWaterCoolerEnabled, isCoolerConnected, moduleToggles, capabilities }))
            .sort((a, b) => a.order - b.order).map(slot => ({
              id: slot.id, name: slot.name, icon: <slot.icon size={14} className={slot.iconClass} />,
              className: slot.tabClassName, badge: slot.renderBadge?.({ status, isWaterCoolerEnabled, isCoolerConnected }),
            })),
          ...tabPlugins.map(plugin => ({ id: plugin.id, name: plugin.name, icon: <Layers size={14} />,
            className: 'tab-plugin-custom', description: plugin.description })),
        ]}
        actions={<>
          {status.oem_installed && status.oem_running && <button className="toggle-btn btn-warning-solid"
            onClick={() => void handleTakeoverOem()} disabled={isSwitchingService}>
            {isSwitchingService ? <Loader2 size={11} className="animate-spin" /> : <Shield size={11} />}接管官方控制台
          </button>}
          <button className="toggle-btn" onClick={onCollapseDashboard} title="折叠为迷你抽屉模式"><Minimize2 size={12} />迷你模式</button>
          <button className="toggle-btn" onClick={handleToggleMaximize} title={isMaximized ? '还原窗口' : '最大化'}>
            {isMaximized ? <Copy size={12} /> : <Square size={12} />}{isMaximized ? '窗口' : '全屏'}
          </button>
          <button className="toggle-btn" onClick={onMinimize} title="最小化"><Minus size={16} /></button>
          <button className="toggle-btn" onClick={onClose} title="关闭窗口"><X size={16} /></button>
        </>}
      />

      {/* Main Body Viewport Slot */}
      <div className="shell-viewport" data-slot="viewport">
        {activeTab === 'overview' && (
          <ModuleErrorBoundary moduleName="状态概览">
            <OverviewView
              status={status}
              capabilities={capabilities}
              osdConfig={osdConfig}
              onUpdateOsdConfig={handleUpdateOsdConfig}
              onSetPowerMode={onSetPowerMode}
              onToggleFanBoost={onToggleFanBoost}
              onToggleBatteryLimit={onToggleBatteryLimit}
              onSwitchRefreshRate={onSwitchRefreshRate}
              onOpenCustomTuning={() => setIsCustomTuningOpen(true)}
              checkOemGuard={checkOemGuard}
              profileVersion={profileVersion}
            />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'gpu' && (
          <ModuleErrorBoundary moduleName="显卡模式" onNavigateToOverview={() => setActiveTab('overview')}>
            <GpuModeView />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'tuning' && (
          <ModuleErrorBoundary moduleName="电源管理" onNavigateToOverview={() => setActiveTab('overview')}>
            <PowerTuningView />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'lighting' && (
          <ModuleErrorBoundary moduleName="RGB 灯效工坊" onNavigateToOverview={() => setActiveTab('overview')}>
            <LightingStudio supportFlags={status.support_flags} />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'water' && isWaterCoolerEnabled && (
          <ModuleErrorBoundary moduleName="水冷魔盒 (BLE)" onNavigateToOverview={() => setActiveTab('overview')}>
            <WaterCoolerView
              status={status}
              onConnectionChange={setIsCoolerConnected}
            />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'system' && (
          <ModuleErrorBoundary moduleName="系统设置" onNavigateToOverview={() => setActiveTab('overview')}>
            <SystemSettingsView
              status={status}
              osdConfig={osdConfig}
              onUpdateOsdConfig={handleUpdateOsdConfig}
              onToggleWinKey={onToggleWinKey}
              onToggleAutostart={onToggleAutostart}
              onToggleWaterCooler={(enabled) => setLocalWaterCoolerEnabled(enabled)}
              moduleToggles={moduleToggles}
              onToggleModule={handleToggleModule}
              capabilities={capabilities}
              previewMode={previewMode}
              onPreviewFormFactor={handlePreviewFormFactor}
            />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'plugins' && (
          <ModuleErrorBoundary moduleName="插件管理" onNavigateToOverview={() => setActiveTab('overview')}>
            <PluginsView
              status={status}
              moduleToggles={moduleToggles}
              onToggleModule={handleToggleModule}
              waterCoolerEnabled={isWaterCoolerEnabled}
              onToggleWaterCooler={(enabled) => {
                setLocalWaterCoolerEnabled(enabled);
                setModuleToggles((prev) => ({ ...prev, water: enabled }));
              }}
              activeSkin={activeSkin}
              onApplySkin={onApplySkin}
              activeBackground={activeBackground}
              onApplyBackground={onApplyBackground}
              onCustomPluginsUpdated={loadTabPlugins}
            />
          </ModuleErrorBoundary>
        )}

        {activeTab === 'sponsor' && (
          <ModuleErrorBoundary moduleName="支持作者" onNavigateToOverview={() => setActiveTab('overview')}>
            <SponsorView />
          </ModuleErrorBoundary>
        )}

        {/* 第三方 Tab 插件渲染沙盒 */}
        {tabPlugins.some((p) => p.id === activeTab) && (
          <ModuleErrorBoundary
            moduleName={tabPlugins.find((p) => p.id === activeTab)?.name || '扩展模块'}
            onNavigateToOverview={() => setActiveTab('overview')}
          >
            <PluginTabHost
              plugin={tabPlugins.find((p) => p.id === activeTab)!}
              status={status}
            />
          </ModuleErrorBoundary>
        )}
      </div>

      {/* Global Custom Tuning Modal Overlay */}
      {isCustomTuningOpen && (
        <CustomTuningPanel
          status={status}
          capabilities={capabilities}
          onToggleFanBoost={onToggleFanBoost}
          onClose={() => {
            setIsCustomTuningOpen(false);
            setProfileVersion((v) => v + 1);
          }}
          onApplied={() => {
            setProfileVersion((v) => v + 1);
          }}
        />
      )}

      {/* OEM Running Modal Alert */}
      {showOemModal && (
        <div className="sys-modal-backdrop" onClick={() => setShowOemModal(false)}>
          <div className="sys-modal-card" onClick={(e) => e.stopPropagation()}>
            <div className="sys-modal-header">
              <div className="sys-modal-icon-badge">
                <Shield size={24} />
              </div>
              <div className="sys-modal-title">官方控制中心正在运行</div>
            </div>
            <div className="sys-modal-body">
              检测到机械革命官方电竞后台正在运行。为防止与官方争抢主板 EC 寄存器并导致<strong>屏幕持续闪烁</strong>，当前处于<strong>安全只读避让模式</strong>。
            </div>
            <div className="sys-modal-actions">
              <button
                onClick={handleTakeoverOem}
                disabled={isSwitchingService}
                className="toggle-btn btn-warning-solid"
              >
                {isSwitchingService ? <Loader2 size={15} className="animate-spin" /> : <Zap size={15} />}
                <span>{isSwitchingService ? '正在接管服务并释放硬件...' : '立即一键接管官方服务'}</span>
              </button>
              <button
                onClick={() => setShowOemModal(false)}
                className="toggle-btn"
              >
                暂不接管，保持只读监控
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
};
