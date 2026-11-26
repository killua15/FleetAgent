import * as React from 'react';
import { cn } from '@/lib/utils';

/** Talmo's `chip`: a pressable filter. Pressed fills with the accent. */
export function Chip({
  pressed,
  count,
  className,
  children,
  ...props
}: React.ComponentProps<'button'> & { pressed?: boolean; count?: number }) {
  return (
    <button
      type="button"
      aria-pressed={pressed}
      data-slot="chip"
      className={cn(
        'inline-flex h-8 flex-none items-center gap-1.5 rounded-[9px] bg-[var(--card)] px-3 text-[length:var(--fs-sm)] font-semibold whitespace-nowrap text-[var(--mut)] shadow-[inset_0_0_0_1px_var(--line)] hover:text-[var(--txt)] disabled:opacity-45 aria-pressed:bg-[var(--acc)] aria-pressed:text-[var(--acc-ink)] aria-pressed:shadow-none pointer-coarse:min-h-10 [&_svg]:size-[15px]',
        className,
      )}
      {...props}
    >
      {children}
      {count !== undefined && (
        <span className="grid h-[18px] min-w-[18px] place-items-center rounded-full bg-black/10 px-1 text-[length:var(--fs-2xs)] font-bold">
          {count}
        </span>
      )}
    </button>
  );
}

export function ChipGroup({ className, ...props }: React.ComponentProps<'div'>) {
  return <div data-slot="chip-group" className={cn('flex flex-wrap gap-1.5', className)} {...props} />;
}
