/**
 * git credential helper started by git for the hub's own git runs (see git-auth.ts).
 * Answers `get` from HUB_GIT_CREDENTIALS ({host: {username, password}}), which the hub
 * puts only into the environment of that git process. `store` / `erase` are ignored:
 * logins are managed in the hub settings. No imports — it starts on every request.
 */
if (process.argv[2] === 'get') {
  let input = '';
  for await (const chunk of process.stdin) input += chunk;
  const fields: Record<string, string> = {};
  for (const line of input.split('\n')) {
    const i = line.indexOf('=');
    if (i > 0) fields[line.slice(0, i)] = line.slice(i + 1);
  }
  if (fields.protocol === 'https' || fields.protocol === 'http') {
    const host = (fields.host ?? '').toLowerCase().replace(/:\d+$/, '');
    let creds: Record<string, { username: string; password: string }> = {};
    try {
      creds = JSON.parse(process.env.HUB_GIT_CREDENTIALS ?? '{}');
    } catch {
      // malformed: answer nothing, git reports the access error itself
    }
    const c = Object.hasOwn(creds, host) ? creds[host] : undefined;
    if (c) process.stdout.write(`username=${c.username}\npassword=${c.password}\n`);
  }
}

export {};
