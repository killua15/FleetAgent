import * as React from 'react';
import { DropdownMenu as DropdownMenuPrimitive } from 'radix-ui';

import { cn } from '@/lib/utils';

/**
 * The overflow menu the narrow-window top bar collapses into (`AppTopBar`) —
 * same wrapping pattern as `tooltip.tsx`: a thin styled layer over radix-ui's
 * own primitives, nothing reimplemented.
 */
function DropdownMenu({ ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Root>) {
  return <DropdownMenuPrimitive.Root data-slot="dropdown-menu" {...props} />;
}

function DropdownMenuTrigger({ ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Trigger>) {
  return <DropdownMenuPrimitive.Trigger data-slot="dropdown-menu-trigger" {...props} />;
}

function DropdownMenuContent({ className, sideOffset = 6, align = 'end', ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Content>) {
  return (
    <DropdownMenuPrimitive.Portal>
      <DropdownMenuPrimitive.Content
        data-slot="dropdown-menu-content"
        sideOffset={sideOffset}
        align={align}
        className={cn(
          'z-50 min-w-[220px] max-w-[calc(100vw-20px)] p-1.5 text-[length:var(--fs-md)] rounded-[14px] bg-[var(--overlay)] text-[var(--txt)] shadow-[var(--shadow-lg),inset_0_0_0_1px_var(--overlay-line)] duration-200 ease-[var(--ease)] data-open:animate-in data-open:fade-in-0 data-open:zoom-in-95 data-closed:animate-out data-closed:fade-out-0 data-closed:zoom-out-95',
          className,
        )}
        {...props}
      />
    </DropdownMenuPrimitive.Portal>
  );
}

function DropdownMenuItem({ className, ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Item>) {
  return (
    <DropdownMenuPrimitive.Item
      data-slot="dropdown-menu-item"
      className={cn(
        'flex h-[38px] cursor-pointer items-center gap-2.5 rounded-[9px] px-2.5 font-semibold whitespace-nowrap outline-none select-none [&_svg]:size-[18px] [&_svg]:text-[var(--mut)] data-highlighted:bg-[var(--card-2)] data-highlighted:text-[var(--txt)] data-highlighted:[&_svg]:text-[var(--acc)] pointer-coarse:h-11',
        className,
      )}
      {...props}
    />
  );
}

function DropdownMenuSeparator({ className, ...props }: React.ComponentProps<typeof DropdownMenuPrimitive.Separator>) {
  return <DropdownMenuPrimitive.Separator data-slot="dropdown-menu-separator" className={cn('mx-1 my-1.5 h-px bg-[var(--line)]', className)} {...props} />;
}

export { DropdownMenu, DropdownMenuTrigger, DropdownMenuContent, DropdownMenuItem, DropdownMenuSeparator };
