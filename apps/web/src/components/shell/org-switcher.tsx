"use client";

import { useState } from "react";
import { Building2, Check, X } from "lucide-react";
import { useTenant } from "@/lib/tenant-context";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

/**
 * There is no "list organizations" API anywhere in the backend -- a tenant
 * is just a free-form string the caller supplies to heavy-api (see
 * ConnectRequest.tenant), not a registered/enumerable entity. So this is a
 * functional "set the tenant id you want to scope to" control, not a fake
 * dropdown of org names the design mockup showed -- rendering a hardcoded
 * or invented org list would be exactly the kind of fabricated data this
 * overhaul is committed to avoiding.
 *
 * Renders nothing when heavy tier isn't deployed here at all (distinct
 * from "no tenant picked yet", which still shows the control).
 */
export function OrgSwitcher() {
  const { tenant, setTenant, heavyTierAvailable } = useTenant();
  const [open, setOpen] = useState(false);
  const [draft, setDraft] = useState(tenant ?? "");

  if (heavyTierAvailable === false) return null;

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (next) setDraft(tenant ?? "");
      }}
    >
      <PopoverTrigger asChild>
        <Button variant="outline" className="w-full justify-start gap-2 text-section">
          <Building2 className="size-4 text-ink-500" />
          <span className="truncate">{tenant ?? "No organization"}</span>
        </Button>
      </PopoverTrigger>
      <PopoverContent align="start" className="w-64 space-y-2">
        <p className="text-label uppercase tracking-wide text-ink-500">Organization id</p>
        <Input
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder="e.g. acme-corp"
          onKeyDown={(e) => {
            if (e.key === "Enter" && draft.trim()) {
              setTenant(draft.trim());
              setOpen(false);
            }
          }}
        />
        <div className="flex gap-2">
          <Button
            size="sm"
            className="flex-1 gap-1"
            disabled={!draft.trim()}
            onClick={() => {
              setTenant(draft.trim());
              setOpen(false);
            }}
          >
            <Check className="size-3.5" />
            Set
          </Button>
          {tenant && (
            <Button
              size="sm"
              variant="outline"
              className="gap-1"
              onClick={() => {
                setTenant(null);
                setOpen(false);
              }}
            >
              <X className="size-3.5" />
              Clear
            </Button>
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}
