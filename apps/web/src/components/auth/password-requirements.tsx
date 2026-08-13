import { Check, Circle } from "lucide-react";
import { PASSWORD_REQUIREMENTS } from "@/lib/auth/validation";
import { cn } from "@/lib/utils";

// Live, proactive feedback rather than a submit-time-only error -- NN/g and
// current password-field UX guidance both call for requirements to be
// visible upfront and checked in real time, not discovered by trial and
// error on submit. Three visual states per item: unmet-neutral (haven't
// tried submitting yet), met (green check, as soon as it's satisfied,
// live), unmet-error (still red only after a submit attempt -- don't shame
// the user for a requirement they haven't gotten to yet).
export function PasswordRequirementsList({ password, showUnmetAsError }: { password: string; showUnmetAsError: boolean }) {
  return (
    <ul className="space-y-1 pt-0.5">
      {PASSWORD_REQUIREMENTS.map((requirement) => {
        const met = requirement.test(password);
        return (
          <li key={requirement.id} className={cn("flex items-center gap-1.5 text-body", met ? "text-health-healthy" : showUnmetAsError ? "text-destructive" : "text-ink-500")}>
            {met ? <Check className="size-3.5 shrink-0" /> : <Circle className="size-3.5 shrink-0" />}
            {requirement.label}
          </li>
        );
      })}
    </ul>
  );
}
