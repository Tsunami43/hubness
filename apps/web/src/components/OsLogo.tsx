import {
  siAlmalinux,
  siAlpinelinux,
  siApple,
  siArchlinux,
  siArtixlinux,
  siAsahilinux,
  siCachyos,
  siCentos,
  siDebian,
  siDeepin,
  siElementary,
  siEndeavouros,
  siFedora,
  siFreebsd,
  siGarudalinux,
  siGentoo,
  siKalilinux,
  siKubuntu,
  siLinux,
  siLinuxmint,
  siManjaro,
  siMxlinux,
  siNixos,
  siOpenbsd,
  siOpensuse,
  siPopos,
  siRaspberrypi,
  siRedhat,
  siRockylinux,
  siSlackware,
  siSolus,
  siUbuntu,
  siVoidlinux,
  siZorin,
  type SimpleIcon,
} from 'simple-icons';

/**
 * Distribution / OS logo for System → Overview (simple-icons, CC0, brand color from the
 * package data). Unknown distributions fall back to their ID_LIKE parent, then to Tux.
 */
const BY_ID: Record<string, SimpleIcon> = {
  arch: siArchlinux,
  archarm: siArchlinux,
  artix: siArtixlinux,
  endeavouros: siEndeavouros,
  manjaro: siManjaro,
  'manjaro-arm': siManjaro,
  garuda: siGarudalinux,
  cachyos: siCachyos,
  debian: siDebian,
  raspbian: siRaspberrypi,
  ubuntu: siUbuntu,
  kubuntu: siKubuntu,
  linuxmint: siLinuxmint,
  pop: siPopos,
  elementary: siElementary,
  zorin: siZorin,
  deepin: siDeepin,
  mx: siMxlinux,
  kali: siKalilinux,
  fedora: siFedora,
  'fedora-asahi-remix': siAsahilinux,
  rhel: siRedhat,
  centos: siCentos,
  rocky: siRockylinux,
  almalinux: siAlmalinux,
  opensuse: siOpensuse,
  'opensuse-leap': siOpensuse,
  'opensuse-tumbleweed': siOpensuse,
  suse: siOpensuse,
  gentoo: siGentoo,
  nixos: siNixos,
  alpine: siAlpinelinux,
  void: siVoidlinux,
  slackware: siSlackware,
  solus: siSolus,
  macos: siApple,
  freebsd: siFreebsd,
  openbsd: siOpenbsd,
};

export function osIcon(id: string, like: string[] = []): SimpleIcon {
  return BY_ID[id] ?? like.map((l) => BY_ID[l]).find(Boolean) ?? siLinux;
}

/** Near-black brand colors (Apple, Tux) would vanish on dark themes: use the text color. */
function visible(hex: string): boolean {
  const n = parseInt(hex, 16);
  const [r, g, b] = [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 60;
}

export function OsLogo({ id, like, className = 'size-24' }: { id: string; like?: string[]; className?: string }) {
  const icon = osIcon(id, like);
  return (
    <svg role="img" viewBox="0 0 24 24" aria-label={icon.title} className={`${className} shrink-0`} fill={visible(icon.hex) ? `#${icon.hex}` : 'currentColor'}>
      <path d={icon.path} />
    </svg>
  );
}
