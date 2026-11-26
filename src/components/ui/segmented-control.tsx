import * as React from 'react';
import { cn } from '@/lib/utils';
import { useSlidingIndicator } from '@/lib/motion';

export interface SegmentedOption<T extends string> {
  value: T;
  label: React.ReactNode;
  title?: string;
  disabled?: boolean;
}

/**
 * Talmo's `seg`: a well with a thumb that slides to the chosen option. Each
 * option is a real button with `aria-pressed`, so it reads and tabs like the
 * button row it replaces.
 */
export function SegmentedControl<T extends string>({
  value,
  options,
  onChange,
  accent = false,
  size = 'default',
  className,
  'aria-label': ariaLabel,
}: {
  value: T;
  options: SegmentedOption<T>[];
  onChange: (value: T) => void;
  /** The thumb in the accent colour instead of a raised surface. */
  accent?: boolean;
  size?: 'default' | 'sm';
  className?: string;
  'aria-label'?: string;
}) {
  const ref = React.useRef<HTMLDivElement>(null);
  const thumb = React.useRef<HTMLSpanElement>(null);
  useSlidingIndicator(ref, thumb, value);

  return (
    <div
      ref={ref}
      role="group"
      aria-label={ariaLabel}
      data-slot="segmented-control"
      className={cn(
        'relative inline-flex rounded-[10px] bg-[var(--card-2)] p-[3px] shadow-[inset_0_0_0_1px_var(--line)]',
        className,
      )}
    >
      <span
        ref={thumb}
        aria-hidden
        className={cn(
          'pointer-events-none absolute top-0 left-0 rounded-[8px] opacity-0',
          accent ? 'bg-[var(--acc)]' : 'bg-[var(--raised)] shadow-[0_1px_2px_rgba(0,0,0,0.18)]',
        )}
      />
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            title={o.title}
            disabled={o.disabled}
            aria-pressed={on}
            onClick={() => onChange(o.value)}
            className={cn(
              'relative z-[1] inline-flex items-center justify-center gap-1.5 rounded-[8px] font-semibold whitespace-nowrap text-[var(--mut)] hover:text-[var(--txt)] disabled:opacity-45 [&_svg]:size-4',
              size === 'sm' ? 'h-6 px-2.5 text-[length:var(--fs-xs)]' : 'h-7 px-[11px] text-[length:var(--fs-sm)]',
              on && (accent ? 'text-[var(--acc-ink)] hover:text-[var(--acc-ink)]' : 'text-[var(--txt)]'),
            )}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}
