/**
 * A server the hub can reach over SSH with a key it holds (Settings → SSH). The private key
 * never leaves the server; `publicKey` goes into the server's `authorized_keys`.
 */
export interface SshConnection {
  id: string;
  /** The alias agents use: `ssh <name>`. */
  name: string;
  host: string;
  port: number;
  username: string;
  /** Free label: dev, staging, prod… (shown to people and agents). */
  environment: string;
  /** What the server is for — the agent reads it to tell prod from dev. */
  description: string;
  publicKey: string;
  /** `known_hosts` lines pinned by the first successful test (empty until then). */
  hostKeys: string;
  createdAt: string;
  updatedAt: string;
}

/** `POST /api/ssh`; `privateKey` imports an existing key (no passphrase), else the hub generates one. */
export interface SshConnectionInput {
  name: string;
  host: string;
  port?: number;
  username: string;
  environment?: string;
  description?: string;
  privateKey?: string;
}

/** A connection attached to a project or a group, with what it is for there. */
export interface SshBinding {
  connection: SshConnection;
  note: string;
}

/** `POST /api/ssh/:id/test`. */
export interface SshTestResult {
  ok: boolean;
  output: string;
  connection: SshConnection;
}
