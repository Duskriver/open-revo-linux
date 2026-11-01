/** Shared control shapes used by the UI, independent of host/platform DTOs. */
export type PowerMode = 1 | 2 | 3 | 4;

export interface FanCurvePoint {
  temp: number;
  duty: number;
}

interface ModeLabel {
  id: PowerMode;
  name: string;
  nameEn: string;
  desc: string;
}

/** Display labels only. Power limits always come from the backend's snapshot. */
export const POWER_MODES: readonly ModeLabel[] = [
  { id: 1, name: '办公模式', nameEn: 'OFFICE / SILENT', desc: '轻载静音长续航，温控保守兼顾静音体验' },
  { id: 2, name: '均衡模式', nameEn: 'BALANCE / GAMING', desc: '日常娱乐与工作，平稳释放兼顾流畅散热' },
  { id: 3, name: '狂暴模式', nameEn: 'BEAST / EXTREME', desc: '原厂高能释放，强效动力尽享峰值性能' },
  { id: 4, name: '自定义模式', nameEn: 'CUSTOM / PROFILE', desc: '专属极客调优与风扇曲线，深度释放性能潜能' },
];
