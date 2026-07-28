import { getCurrentWindow } from '@tauri-apps/api/window';

/** How transparent the window goes when you're not in it, 0-1, where 1 is the
 * focused tint (no extra fade). Stored separately from the theme because it's a
 * different kind of preference: the theme is what Hangar looks like, this is how
 * much of your desktop you want to see through it. */
const UNFOCUSED_KEY = 'hangar.glass.unfocused';

export const DEFAULT_UNFOCUSED_TINT = 0.38;

export function readUnfocusedTint(): number {
  const raw = Number(window.localStorage.getItem(UNFOCUSED_KEY));
  return Number.isFinite(raw) && raw > 0 && raw <= 1 ? raw : DEFAULT_UNFOCUSED_TINT;
}

export function writeUnfocusedTint(value: number) {
  window.localStorage.setItem(UNFOCUSED_KEY, String(value));
}

/**
 * Turns the real macOS window translucency on and off.
 *
 * Two separate things have to line up, and either alone looks broken:
 *
 * - `transparent: true` in tauri.conf plus `macOSPrivateApi`, which is what
 *   lets anything behind the window show through at all. Without it the glass
 *   tokens are just a white wash over an opaque background — which is what
 *   "glass" was before this.
 * - A vibrancy material, so what shows through is a blurred, desaturated
 *   backdrop rather than a literal picture of your desktop with text on top.
 *   `underWindowBackground` sits behind the web content, which is the one that
 *   composites correctly with our own translucent tints.
 *
 * Non-glass themes clear the effect and paint an opaque background, so the
 * transparent window is invisible to them.
 */
export async function applyWindowGlass(enabled: boolean): Promise<void> {
  const win = getCurrentWindow();
  try {
    if (enabled) {
      await win.setEffects({ effects: ['underWindowBackground'] as never, radius: 12 });
    } else {
      await win.clearEffects();
    }
  } catch {
    // Not macOS, or the platform refused the material. The CSS tints still
    // work — you get flat translucency instead of frosted, which is a
    // downgrade, not a failure worth interrupting anyone over.
  }
}
