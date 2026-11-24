import { siBitbucket, siCodeberg, siGit, siGitea, siGithub, siGitlab, type SimpleIcon } from 'simple-icons';
import type { GitProviderId } from '@hubness/shared';

/**
 * Git hosting logo (simple-icons, CC0). The color is the brand one from the package
 * data; GitHub uses the text color (its black logo is invisible on a dark theme).
 * Self-hosted Gitea/Codeberg instances are recognized by the remote host.
 */
function iconFor(provider: GitProviderId | null | undefined, url?: string | null): { icon: SimpleIcon; brand: boolean } | null {
  if (provider === 'github') return { icon: siGithub, brand: false };
  if (provider === 'gitlab') return { icon: siGitlab, brand: true };
  if (provider === 'bitbucket') return { icon: siBitbucket, brand: true };
  if (!provider) return null;
  const host = url?.toLowerCase() ?? '';
  if (host.includes('codeberg')) return { icon: siCodeberg, brand: true };
  if (host.includes('gitea') || host.includes('forgejo')) return { icon: siGitea, brand: true };
  return { icon: siGit, brand: true };
}

export function ProviderIcon({ provider, url, className = 'size-4' }: { provider: GitProviderId | null | undefined; url?: string | null; className?: string }) {
  const found = iconFor(provider, url);
  if (!found) return null;
  return (
    <svg role="img" viewBox="0 0 24 24" aria-label={found.icon.title} className={`${className} shrink-0`} fill={found.brand ? `#${found.icon.hex}` : 'currentColor'}>
      <path d={found.icon.path} />
    </svg>
  );
}
