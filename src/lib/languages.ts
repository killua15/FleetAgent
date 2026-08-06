/**
 * The languages an agent can be asked to work in.
 *
 * Deliberately a short list, not every ISO 639-1 code: this is a dropdown
 * someone reads, and 180 entries is a search box pretending to be a choice.
 * These are the widest-spoken languages plus the ones that actually show up in
 * software teams — enough that almost nobody has to type their own, and the
 * `label` is free text on the way to the CLI anyway, so an unlisted one is a
 * feature request rather than a wall.
 *
 * `label` is what reaches the agent. It is the language's **own** name because
 * that is what the instruction ends up saying — "Responde siempre en español"
 * reads as a native instruction, "Responde siempre en Spanish" reads as a
 * string that was concatenated.
 */
export interface LanguageOption {
  /** BCP 47 prefix, used only to guess a default from the OS. */
  code: string;
  /** The language's own name — this is the value stored and sent to the CLI. */
  label: string;
}

export const LANGUAGES: LanguageOption[] = [
  { code: 'es', label: 'español' },
  { code: 'en', label: 'English' },
  { code: 'pt', label: 'português' },
  { code: 'fr', label: 'français' },
  { code: 'de', label: 'Deutsch' },
  { code: 'it', label: 'italiano' },
  { code: 'nl', label: 'Nederlands' },
  { code: 'ru', label: 'русский' },
  { code: 'zh', label: '中文' },
  { code: 'ja', label: '日本語' },
  { code: 'ko', label: '한국어' },
  { code: 'hi', label: 'हिन्दी' },
  { code: 'ar', label: 'العربية' },
  { code: 'tr', label: 'Türkçe' },
  { code: 'pl', label: 'polski' },
];

/**
 * The language this machine is already set to, as a starting suggestion.
 *
 * Defaulting to English would be the wrong answer for the person this feature
 * exists for — someone who keeps having to ask for their own language back. An
 * unknown locale falls through to `undefined` rather than guessing, so the UI
 * can show "no preference" instead of silently choosing for them.
 */
export function systemLanguage(): LanguageOption | undefined {
  const tag = typeof navigator === 'undefined' ? '' : navigator.language;
  const prefix = tag.split('-')[0]?.toLowerCase();
  return LANGUAGES.find((l) => l.code === prefix);
}
