import { Check } from "lucide-react";
import { cn } from "@/lib/utils";

export interface WizardStep {
  label: string;
  status: "done" | "active" | "pending";
}

/** Pure presentational breadcrumb-style stepper for the connect-repository wizard -- each route/page owns its own fixed `steps` array (method choice is always 3-4 items depending on the chosen path), this component just renders it. */
export function StepIndicator({ steps }: { steps: WizardStep[] }) {
  return (
    <div className="mb-8 flex items-center gap-2 text-mono-code">
      {steps.map((step, i) => (
        <div key={step.label} className="flex flex-1 items-center gap-2 last:flex-none">
          <div className="flex items-center gap-2">
            <span
              className={cn(
                "flex size-5 shrink-0 items-center justify-center rounded-full text-[11px] font-semibold",
                step.status === "done" && "bg-health-healthy text-canvas",
                step.status === "active" && "bg-primary text-primary-foreground",
                step.status === "pending" && "border border-border-strong text-ink-500",
              )}
            >
              {step.status === "done" ? <Check className="size-3" /> : i + 1}
            </span>
            <span className={cn(step.status === "pending" ? "text-ink-500" : "text-ink-100 font-medium")}>{step.label}</span>
          </div>
          {i < steps.length - 1 && <div className="mx-1 h-px flex-1 bg-border-strong" />}
        </div>
      ))}
    </div>
  );
}
