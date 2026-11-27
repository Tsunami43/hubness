/**
 * How loud an environment label of an SSH connection is: `prod` stands out, staging-like
 * ones warn, the rest stays quiet. Whole words only — "express" is not "pre", "reproduce"
 * is not "prod".
 */
export type EnvTone = 'prod' | 'staging' | 'other';

export function envTone(env: string): EnvTone {
  const words = env.toLowerCase().split(/[^a-z0-9]+/).filter(Boolean);
  const has = (re: RegExp) => words.some((w) => re.test(w));
  // "preprod" is not production yet
  if (has(/^(pre-?prod\w*|stag\w*|stage|test\w*|qa|uat|preview|pre)$/) || /pre[-_ ]prod/.test(env.toLowerCase())) return 'staging';
  if (has(/^(prod|production|live)$/)) return 'prod';
  return 'other';
}

const TONE: Record<EnvTone, string> = { prod: 'bg-danger/15 text-danger', staging: 'bg-warn/15 text-warn', other: 'bg-sunken text-muted' };

/** The classes of an environment badge. */
export const envCls = (env: string) => TONE[envTone(env)];
