import * as React from 'react';
import { cn } from '@/lib/utils';

/** Talmo's `switch`: 42×24, the knob slides with the shared easing. */
export function Switch({
  checked,
  onCheckedChange,
  className,
  disabled,
  ...props
}: Omit<React.ComponentProps<'button'>, 'onChange'> & {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      disabled={disabled}
      data-slot="switch"
      onClick={() => onCheckedChange(!checked)}
      className={cn(
        'relative h-6 w-[42px] flex-none rounded-full bg-[var(--well)] shadow-[inset_0_0_0_1px_var(--line2)] disabled:opacity-45 aria-checked:bg-[var(--acc)] aria-checked:shadow-none',
        className,
      )}
      {...props}
    >
      <i
        aria-hidden
        className={cn(
          'absolute top-[3px] left-[3px] size-[18px] rounded-full bg-white shadow-[0_1px_3px_rgba(0,0,0,0.25)] transition-transform duration-[var(--dur)] ease-[var(--ease)]',
          checked && 'translate-x-[18px]',
        )}
      />
    </button>
  );
}
