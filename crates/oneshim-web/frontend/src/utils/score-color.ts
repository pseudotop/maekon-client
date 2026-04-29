import { dataViz } from '../styles/tokens'

/** Map a 0-100 score to a dataViz stroke color (good/warning/critical). */
export function scoreColor(score: number): string {
  if (score >= 70) return dataViz.stroke.good
  if (score >= 40) return dataViz.stroke.warning
  return dataViz.stroke.critical
}
