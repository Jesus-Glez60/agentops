"use client";

import { useState } from "react";
import { Check, Copy } from "lucide-react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

export function CopyButton({ value, label = "Copy", className }: { value: string; label?: string; className?: string }) {
  const [copied, setCopied] = useState(false);

  async function handleCopy() {
    // `navigator.clipboard` only exists in secure contexts (HTTPS or
    // localhost) -- self-hosted deployments are commonly plain HTTP, where
    // it's simply undefined rather than throwing, so this can't be a bare
    // try/catch around the async API alone. Fall back to the older
    // execCommand("copy") path (works over HTTP, deprecated but still
    // supported everywhere) whenever the modern API isn't there.
    try {
      if (navigator.clipboard) {
        await navigator.clipboard.writeText(value);
      } else {
        const textarea = document.createElement("textarea");
        textarea.value = value;
        textarea.style.position = "fixed";
        textarea.style.opacity = "0";
        document.body.appendChild(textarea);
        textarea.select();
        const ok = document.execCommand("copy");
        document.body.removeChild(textarea);
        if (!ok) throw new Error("execCommand copy failed");
      }
      setCopied(true);
      toast.success("Copied to clipboard");
      setTimeout(() => setCopied(false), 1500);
    } catch {
      toast.error("Couldn't copy -- clipboard access was denied");
    }
  }

  return (
    <Button variant="outline" size="sm" onClick={handleCopy} className={cn("gap-1.5 text-mono-code", className)}>
      {copied ? <Check className="size-3.5" /> : <Copy className="size-3.5" />}
      {label}
    </Button>
  );
}
