import * as React from "react"

import { cn } from "@/lib/utils"

function Input({ className, type, ...props }: React.ComponentProps<"input">) {
  return (
    <input
      type={type}
      data-slot="input"
      className={cn(
        "h-10 w-full min-w-0 px-3 py-1 file:inline-flex file:h-6 file:border-0 file:bg-transparent file:text-sm file:font-medium file:text-foreground rounded-[11px] border-0 bg-[var(--card)] shadow-[inset_0_0_0_1px_var(--line2)] font-medium outline-none placeholder:text-[var(--dim)] focus-visible:shadow-[inset_0_0_0_1.5px_var(--acc)] focus-visible:ring-3 focus-visible:ring-ring/25 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:shadow-[inset_0_0_0_1.5px_var(--red)] text-[length:var(--fs-base)]",
        className
      )}
      {...props}
    />
  )
}

export { Input }
