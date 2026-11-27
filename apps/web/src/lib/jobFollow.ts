import { useEffect, useRef, useState } from 'react';
import type { JobStreamMsg } from '@hubness/shared';
import { liveStream } from './stream';

/**
 * Follow a terminal job without showing a terminal: its output so far and the exit code
 * (`undefined` while it runs). A (re)connect that finds the job already exited counts as
 * the exit — the exit message itself goes only to the connections open at that moment.
 */
export function useJobFollow(jobId: string | null) {
  const out = useRef('');
  const [output, setOutput] = useState('');
  const [exit, setExit] = useState<number | null | undefined>(undefined);
  useEffect(() => {
    out.current = '';
    setOutput('');
    setExit(undefined);
    if (!jobId) return;
    let done = false;
    const stop = liveStream(`/api/jobs/${jobId}/stream`, (data) => {
      if (done) return;
      const m = JSON.parse(data) as JobStreamMsg;
      if (m.type === 'output') out.current += m.data;
      else if (m.type === 'snapshot') out.current = m.output;
      setOutput(out.current);
      const code = m.type === 'exit' ? m.code : m.type === 'snapshot' && m.job.status === 'exited' ? m.job.exitCode : undefined;
      if (code !== undefined) {
        done = true;
        stop();
        setExit(code);
      }
    });
    return () => {
      done = true;
      stop();
    };
  }, [jobId]);
  return { output, exit };
}

// eslint-disable-next-line no-control-regex
const ANSI = /\x1b\[[0-9;?]*[A-Za-z]|\x1b\][^\x07]*\x07/g;

/** The last lines of a job's output as plain text (lines rewritten with \r keep their last state). */
export function plainTail(out: string, n: number) {
  return out
    .replace(ANSI, '')
    .split('\n')
    .map((l) => l.split('\r').filter(Boolean).pop() ?? '')
    .filter((l) => l.trim())
    .slice(-n);
}
