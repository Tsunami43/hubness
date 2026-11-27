import { Navigate, useParams } from 'react-router';
import { Activity, Boxes, Cog, KeyRound, Package } from 'lucide-react';
import { SubmenuIndex, SubmenuLayout, type SubmenuItem } from '../components/SubmenuLayout';
import { t } from '../i18n';
import { usePlatform } from './system/platform';
import { OverviewSection } from './system/overview';
import { DockerSection } from './system/docker';
import { ServicesSection } from './system/services';
import { PackagesSection } from './system/packages';
import { SshSection } from './system/ssh';

/**
 * System: server monitoring with a submenu and search. The menu asks only the (static)
 * platform; every section loads (and refreshes) its own data only while it is open.
 */
export function SystemLayout() {
  const platform = usePlatform();
  const items: SubmenuItem[] = [
    { to: 'overview', label: t('sys.overview'), hint: t('sys.overviewMenuHint'), icon: <Activity className="size-4 text-accent" />, keywords: ['cpu', 'memory', 'ram', 'disk', 'load', 'uptime', 'swap'] },
    { to: 'docker', label: 'Docker', hint: t('sys.dockerMenuHint'), icon: <Boxes className="size-4 text-accent" />, keywords: ['containers', 'compose', 'logs', 'images'] },
    { to: 'services', label: t('sys.services'), hint: platform ? (platform.services ?? t('sys.unavailable')) : '', icon: <Cog className="size-4 text-accent" />, keywords: ['systemd', 'launchd', 'launchctl', 'units', 'agents', 'journal', 'logs', 'failed'] },
    { to: 'ssh', label: 'SSH', hint: t('ssh.menuHint'), icon: <KeyRound className="size-4 text-accent" />, keywords: ['ssh', 'keys', 'servers', 'deploy', 'prod', 'dev', 'authorized_keys'] },
    { to: 'packages', label: t('sys.packages'), hint: t('sys.packagesMenuHint'), icon: <Package className="size-4 text-accent" />, keywords: ['pacman', 'aur', 'yay', 'paru', 'cargo', 'rustup', 'npm', 'pip', 'uv', 'go', 'apt', 'dnf', 'flatpak', 'snap', 'brew', 'updates', 'installed', 'upgrade'] },
  ];
  return <SubmenuLayout base="/system" title={t('nav.system')} items={items} searchPlaceholder={t('sys.search')} />;
}

export function SystemIndex() {
  return <SubmenuIndex to="overview" />;
}

export function SystemSection() {
  const { section } = useParams();
  switch (section) {
    case 'overview':
      return <OverviewSection />;
    case 'docker':
      return <DockerSection />;
    case 'services':
      return <ServicesSection />;
    case 'packages':
      return <PackagesSection />;
    case 'ssh':
      return <SshSection />;
    default:
      return <Navigate to="/system" replace />;
  }
}
