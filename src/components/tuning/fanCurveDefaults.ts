import type { FanCurvePoint } from '../../types/controls';

export const DEFAULT_8_POINT_CURVE: FanCurvePoint[] = [
  { temp: 40, duty: 22 },
  { temp: 50, duty: 30 },
  { temp: 60, duty: 45 },
  { temp: 68, duty: 55 },
  { temp: 75, duty: 65 },
  { temp: 82, duty: 75 },
  { temp: 88, duty: 85 },
  { temp: 95, duty: 100 },
];

export function normalizeFanCurve8(curve?: FanCurvePoint[], minDuty: number = 0): FanCurvePoint[] {
  if (!curve || curve.length !== 8) {
    return DEFAULT_8_POINT_CURVE.map((p) => ({ ...p, duty: Math.max(minDuty, p.duty) }));
  }
  return curve.map((p) => ({ ...p, duty: Math.max(minDuty, p.duty) }));
}
