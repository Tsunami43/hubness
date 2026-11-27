/**
 * Copy text. `navigator.clipboard` exists only in secure contexts — not on a hub opened over
 * plain http from another machine — so there is a fallback through a hidden textarea.
 * Resolves to whether the text was copied: callers show it.
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard && window.isSecureContext) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // denied or unavailable: try the old way
  }
  const area = document.createElement('textarea');
  area.value = text;
  area.setAttribute('readonly', '');
  area.style.cssText = 'position:fixed;top:0;left:0;width:1px;height:1px;opacity:0';
  document.body.appendChild(area);
  const active = document.activeElement as HTMLElement | null;
  try {
    area.select();
    area.setSelectionRange(0, text.length);
    return document.execCommand('copy');
  } catch {
    return false;
  } finally {
    area.remove();
    active?.focus?.();
  }
}
