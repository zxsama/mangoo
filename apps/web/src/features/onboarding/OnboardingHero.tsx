import { canPickNativeFolder, pickNativeFolder } from "../../core/desktop";
import { LiquidGlassButton } from "../../design/liquid-glass";

export interface OnboardingHeroProps {
  rootCount: number;
  loading: boolean;
  onAddRoot: (path: string) => Promise<void> | void;
}

/**
 * Empty-state hero shown when the library has no roots configured.
 * Either fires the native folder picker (Electron) or focuses the existing
 * root-path input in `LibrarySidebar` (browser fallback) so onboarding stays
 * inside the same shell.
 */
export function OnboardingHero({ rootCount, loading, onAddRoot }: OnboardingHeroProps) {
  if (loading || rootCount > 0) {
    return null;
  }

  async function chooseFolder() {
    if (canPickNativeFolder()) {
      const folder = await pickNativeFolder();
      if (folder) {
        await onAddRoot(folder);
        return;
      }
    }
    const input = document.querySelector<HTMLInputElement>('[aria-label="Root path"]');
    if (!input) return;
    input.scrollIntoView({ behavior: "smooth", block: "center" });
    input.focus();
    input.select();
  }

  return (
    <section className="workspace simple-workspace onboarding-hero" aria-label="Onboarding">
      <div className="onboarding-hero-content">
        <h2 className="onboarding-hero-title">Add a folder to get started</h2>
        <p className="onboarding-hero-copy">
          Mangoo indexes existing folders without copying their contents. Point it at a
          local directory and your media will start showing up here.
        </p>
        <LiquidGlassButton
          className="onboarding-hero-cta"
          onClick={() => void chooseFolder()}
          tone="primary"
          type="button"
        >
          Choose folder
        </LiquidGlassButton>
      </div>
    </section>
  );
}
