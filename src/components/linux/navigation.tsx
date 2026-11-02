import { BatteryCharging, Cpu, Droplets, Gauge, Heart, Layers, Palette } from 'lucide-react';
import type { DashboardTab } from '../common/DashboardChrome';

/** Keep Linux navigation independent of the Windows plugin runtime and model schema. */
export const LINUX_TABS: DashboardTab[] = [
  { id: 'overview', name: '状态总览', className: 'tab-overview', icon: <Gauge size={14} /> },
  { id: 'gpu', name: '显卡模式', className: 'tab-gpu', icon: <Cpu size={14} /> },
  { id: 'tuning', name: '电源管理', className: 'tab-tuning', icon: <BatteryCharging size={14} /> },
  { id: 'lighting', name: 'RGB 灯效工坊', className: 'tab-lighting', icon: <Palette size={14} /> },
  { id: 'water', name: '水冷魔盒 (BLE)', className: 'tab-water', icon: <Droplets size={14} /> },
  { id: 'system', name: '系统', className: 'tab-system', icon: <Layers size={14} /> },
  { id: 'sponsor', name: '支持作者', className: 'tab-sponsor', icon: <Heart size={14} /> },
];

export function linuxTabs(capabilities: { id: string; available: boolean }[] = []) {
  return LINUX_TABS.filter(tab => tab.id !== 'water' || capabilities.some(cap => cap.id === 'water' && cap.available));
}
