/** Scenarios kept for context but never treated as a qualified primary. */
export const REFERENCE_ONLY_SCENARIO_IDS = new Set(['bullish_bounce']);

export function isReferenceOnlyScenario(id?: string | null): boolean {
  return REFERENCE_ONLY_SCENARIO_IDS.has((id ?? '').trim().toLowerCase());
}
