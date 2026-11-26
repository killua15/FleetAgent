import * as React from 'react';
import { cn } from '@/lib/utils';

/** Talmo's `phead`: a large title, a line of context under it, and the page's
 * actions to the right — wrapping under the title on narrow windows. */
export function PageHeader({
  title,
  subtitle,
  eyebrow,
  actions,
  className,
  children,
}: {
  title: React.ReactNode;
  subtitle?: React.ReactNode;
  eyebrow?: React.ReactNode;
  actions?: React.ReactNode;
  className?: string;
  children?: React.ReactNode;
}) {
  return (
    <header
      data-slot="page-header"
      className={cn('mt-1.5 mb-4 flex flex-wrap items-end gap-4 max-[760px]:mb-3', className)}
    >
      <div className="min-w-0 flex-[1_1_260px]">
        {eyebrow && (
          <div className="mb-1 text-[length:var(--fs-xs)] font-semibold tracking-[0.04em] text-[var(--mut)] uppercase">{eyebrow}</div>
        )}
        <h1 className="text-[clamp(26px,2.4vw,34px)] leading-[1.06] font-semibold tracking-[-0.025em] text-[var(--ink)] max-[760px]:text-[26px]">
          {title}
        </h1>
        {subtitle && <p className="mt-2 max-w-[70ch] text-[length:var(--fs-base)] text-[var(--mut)] max-[760px]:text-[length:var(--fs-md)]">{subtitle}</p>}
      </div>
      {actions && <div className="flex flex-wrap items-center gap-2 max-[760px]:w-full">{actions}</div>}
      {children}
    </header>
  );
}
