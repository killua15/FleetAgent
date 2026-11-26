import * as React from "react"
import { cva, type VariantProps } from "class-variance-authority"
import { Slot } from "radix-ui"

import { cn } from "@/lib/utils"

// Talmo's button family: `btn` (40px, r11, inset hairline), `btn--dark` for the
// one primary action, `btn--ghost`, and `ibtn` — the round icon button
// (`size="icon-round"`). Sizes
// are a step denser than Talmo's (sm 32 / default 38) because Hangar's bars sit
// above terminals and every pixel they take is a terminal row.
const buttonVariants = cva(
  "group/button inline-flex shrink-0 items-center justify-center rounded-[11px] border-0 bg-clip-padding text-[length:var(--fs-md)] font-semibold whitespace-nowrap outline-none select-none focus-visible:ring-3 focus-visible:ring-ring/50 active:not-aria-[haspopup]:translate-y-px disabled:pointer-events-none disabled:opacity-45 aria-invalid:shadow-[inset_0_0_0_1.5px_var(--red)] [&_svg]:pointer-events-none [&_svg]:shrink-0 [&_svg:not([class*='size-'])]:size-4",
  {
    variants: {
      variant: {
        default:
          "bg-primary text-primary-foreground shadow-[0_8px_16px_-10px_color-mix(in_srgb,var(--acc)_70%,transparent)] hover:bg-[color-mix(in_oklch,var(--acc),white_10%)]",
        outline:
          "bg-[var(--card)] text-foreground shadow-[inset_0_0_0_1px_var(--line2)] hover:bg-[var(--card-2)] aria-expanded:bg-[var(--card-2)] aria-pressed:bg-[var(--accD)] aria-pressed:text-[var(--acc)] aria-pressed:shadow-[inset_0_0_0_1px_color-mix(in_srgb,var(--acc)_40%,transparent)]",
        secondary:
          "bg-[var(--card-2)] text-secondary-foreground shadow-[inset_0_0_0_1px_var(--line)] hover:bg-[var(--raised)] aria-expanded:bg-[var(--raised)]",
        ghost:
          "bg-transparent hover:bg-[var(--card-2)] hover:text-foreground aria-expanded:bg-[var(--card-2)] aria-expanded:text-foreground",
        destructive:
          "bg-destructive/12 text-destructive hover:bg-destructive/20 focus-visible:ring-destructive/25",
        link: "text-primary underline-offset-4 hover:underline",
      },
      size: {
        default:
          "h-[38px] gap-2 px-4 has-data-[icon=inline-end]:pr-3 has-data-[icon=inline-start]:pl-3",
        xs: "h-6 gap-1 rounded-[8px] px-2 text-[length:var(--fs-xs)] [&_svg:not([class*='size-'])]:size-3",
        sm: "h-8 gap-1.5 rounded-[9px] px-3 text-[length:var(--fs-sm)] has-data-[icon=inline-end]:pr-2 has-data-[icon=inline-start]:pl-2 [&_svg:not([class*='size-'])]:size-4",
        lg: "h-[46px] gap-2 rounded-[13px] px-5 text-[length:var(--fs-lg)]",
        // Quiet and square: it closes terminals and removes rows, and it takes
        // its colour from the variant — a `ghost` one must stay transparent.
        icon: "size-8 rounded-[9px]",
        // Talmo's `ibtn`: raised and round. A size of its own, because its
        // surface would override any variant's background.
        "icon-round":
          "size-10 rounded-full bg-[var(--raised)] shadow-[inset_0_0_0_1px_var(--line),var(--shadow)]",
        "icon-xs":
          "size-6 rounded-[8px] [&_svg:not([class*='size-'])]:size-3",
        "icon-sm":
          "size-8 rounded-[9px] [&_svg:not([class*='size-'])]:size-4",
        "icon-lg": "size-11 rounded-full",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  }
)

function Button({
  className,
  variant = "default",
  size = "default",
  asChild = false,
  ...props
}: React.ComponentProps<"button"> &
  VariantProps<typeof buttonVariants> & {
    asChild?: boolean
  }) {
  const Comp = asChild ? Slot.Root : "button"

  return (
    <Comp
      data-slot="button"
      data-variant={variant}
      data-size={size}
      className={cn(buttonVariants({ variant, size, className }))}
      {...props}
    />
  )
}

export { Button, buttonVariants }
