"use client";

import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { Textarea } from "@/components/ui/textarea";

/**
 * Reason capture for reducing a gotcha's prominence -- also reused to edit
 * an already-recorded reason (same dialog, pre-filled with `initialReason`).
 * A demotion without a reason would just be an unexplained downranking, so
 * `onSubmit` is only callable with non-empty trimmed text.
 */
export function CurationReasonDialog({
  open,
  onOpenChange,
  initialReason,
  onSubmit,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  initialReason?: string | null;
  onSubmit: (reason: string) => void;
}) {
  const [reason, setReason] = useState(initialReason ?? "");

  // Re-sync when the dialog is (re)opened for a different node/reason --
  // stale text from a previous open must never leak into this one.
  useEffect(() => {
    if (open) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setReason(initialReason ?? "");
    }
  }, [open, initialReason]);

  function submit() {
    const trimmed = reason.trim();
    if (!trimmed) return;
    onSubmit(trimmed);
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Reduce prominence</DialogTitle>
          <DialogDescription>
            This gotcha stays in the library and is still shown everywhere -- it just ranks lower, with this reason attached, so it&apos;s clear why it&apos;s not treated as top-priority knowledge.
          </DialogDescription>
        </DialogHeader>
        <Textarea
          value={reason}
          onChange={(e) => setReason(e.target.value)}
          placeholder="e.g. Only affects old Linux envs without a modern glibc; not relevant on supported platforms."
          rows={4}
          autoFocus
        />
        <DialogFooter>
          <Button variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button onClick={submit} disabled={!reason.trim()}>
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
