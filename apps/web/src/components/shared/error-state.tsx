import { CircleAlert } from "lucide-react";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";

/**
 * Replaces the loading/error markup that was duplicated 5+ times across
 * the old pages with *inconsistent* spacing (mt-4 vs mt-6 between
 * instances, confirmed by audit) -- one component, one spacing decision.
 */
export function ErrorState({ message, title = "Something went wrong" }: { message: string; title?: string }) {
  return (
    <Alert variant="destructive" className="border-health-failed/40 bg-health-failed/5">
      <CircleAlert className="size-4" />
      <AlertTitle>{title}</AlertTitle>
      <AlertDescription>{message}</AlertDescription>
    </Alert>
  );
}
