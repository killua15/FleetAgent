import * as React from "react"

import { cn } from "@/lib/utils"

function Textarea({ className, ...props }: React.ComponentProps<"textarea">) {
  return (
    <textarea
      data-slot="textarea"
      className={cn(
        "flex field-sizing-content min-h-16 w-full px-3 py-2.5 rounded-[11px] border-0 bg-[var(--card)] shadow-[inset_0_0_0_1px_var(--line2)] font-medium outline-none placeholder:text-[var(--dim)] focus-visible:shadow-[inset_0_0_0_1.5px_var(--acc)] focus-visible:ring-3 focus-visible:ring-ring/25 disabled:pointer-events-none disabled:cursor-not-allowed disabled:opacity-50 aria-invalid:shadow-[inset_0_0_0_1.5px_var(--red)] text-[length:var(--fs-base)]",
        className
      )}
      {...props}
    />
  )
}

export { Textarea }
