// Server monitoring: host, metrics, Docker, services, packages, “Ask about this”.

// ---- System: monitoring ----

/** OS family of the hub server: Linux distributions and macOS are supported; others get basics only. */
export type HostPlatform = 'linux' | 'darwin' | 'other';

/** What the System pages can do on this server (asked once by the System menu). */
export interface SystemPlatform {
  platform: HostPlatform;
  /** Service manager behind System → Services; null — the section is unavailable. */
  services: 'systemd' | 'launchd' | null;
}

/** Static host facts for the System → Overview header (re-read every few minutes). */
export interface HostInfo extends SystemPlatform {
  /** os-release ID (arch, ubuntu, …) or `macos`. */
  osId: string;
  /** os-release ID_LIKE, used to pick a logo for derivatives. */
  osLike: string[];
  osName: string;
  arch: string;
  kernel: string;
  /** Machine model (DMI on Linux, hw.model on macOS). */
  host: string | null;
  cpu: string | null;
}

export interface GpuInfo {
  name: string;
  /** Busy percent where the driver exposes it (amdgpu sysfs, nvidia-smi). */
  usagePercent: number | null;
  tempC: number | null;
}

export interface TempSensor {
  kind: 'cpu' | 'gpu' | 'disk' | 'other';
  label: string;
  celsius: number;
}

export interface SystemOverview {
  info: HostInfo;
  hostname: string;
  uptimeSec: number;
  load: [number, number, number];
  cpus: number;
  cpuPercent: number | null;
  gpus: GpuInfo[];
  /** Hardware sensors (Linux hwmon); empty where unreadable (macOS without root). */
  temps: TempSensor[];
  memory: { totalMb: number; availableMb: number; usedPercent: number };
  swap: { totalMb: number; usedMb: number };
  disks: { mount: string; device: string; sizeGb: number; usedGb: number; usedPercent: number }[];
}

export interface DockerNetwork {
  id: string;
  name: string;
  driver: string;
  scope: string;
  subnet: string | null;
  /** Names of attached containers (running ones are attached). */
  containers: string[];
  /** Compose project that created it. */
  project: string | null;
  /** bridge / host / none — Docker's own, can't be removed. */
  builtin: boolean;
  internal: boolean;
  createdAt: string;
}

export interface DockerVolume {
  name: string;
  driver: string;
  /** Containers using it (running or stopped); 0 — unused. */
  links: number;
  size: string | null;
  project: string | null;
  /** Created without a name (hash-like name). */
  anonymous: boolean;
  mountpoint: string;
}

export interface ContainerInfo {
  id: string;
  name: string;
  image: string;
  state: string; // running | exited | restarting…
  status: string; // «Up 9 hours (healthy)»
  health: string | null;
  restartCount: number;
  oomKilled: boolean;
  exitCode: number;
  ports: string;
  memLimitMb: number | null;
  /** When the container stopped (ISO), if it is not running. */
  finishedAt: string | null;
}

export interface UnitInfo {
  name: string;
  scope: 'system' | 'user';
  load: string;
  active: string;
  sub: string;
  description: string;
}

export interface PackageInfo {
  name: string;
  version: string;
  newVersion?: string;
  /** Extra detail: a local path (cargo), «active, default» (rustup), the module (go). */
  note?: string;
}

/** A package manager found on the server (one tab in System → Packages). */
export interface PackageManagerInfo {
  id: string;
  label: string;
  kind: 'system' | 'language' | 'tool';
}

export interface PackagesResponse {
  manager: string;
  label: string;
  installed: number;
  /** The main list: explicitly installed (system managers) or everything the manager installed. */
  packages: PackageInfo[];
  listTitle: 'explicit' | 'installed';
  /** null — this manager can't check for updates (see updatesNote). */
  updates: PackageInfo[] | null;
  updatesNote: string;
  /** When the data was read (it is cached on the server). */
  checkedAt: string;
}

/** Target of "Ask about this". */
export type AskTarget =
  | { kind: 'container'; id: string }
  | { kind: 'unit'; id: string; scope: 'system' | 'user' }
  | { kind: 'package'; id: string };

/** Progress of a self-update started from the web (`POST /api/system/update`). */
export interface HubUpdateState {
  /**
   * idle — none started; running — downloading and replacing the binary; installed — the new
   * binary is in place but this process still runs the old one (restart the hub); failed.
   */
  state: 'idle' | 'running' | 'installed' | 'failed';
  /** The error of a failed update, or the version an installed one put in place. */
  message: string | null;
}

/** `GET /api/system/version`: this hub's version and the latest release. */
export interface HubVersion {
  version: string;
  /** The latest released version; null — unknown (offline, or the check is turned off). */
  latest: string | null;
  updateAvailable: boolean;
  /** Release notes of `latest`, when releases live on a web page. */
  releaseUrl: string | null;
  /** This hub can replace itself (the installed `hubness` binary, not a dev checkout). */
  canUpdate: boolean;
  update: HubUpdateState;
}
