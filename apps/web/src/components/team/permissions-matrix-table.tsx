import { Fragment } from "react";
import { Check, X } from "lucide-react";
import type { Capability, MemberRole } from "@/lib/api/team-api";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";

const ROLE_ORDER: MemberRole[] = ["admin", "member", "viewer", "billing"];
const ROLE_LABELS: Record<MemberRole, string> = { admin: "Admin", member: "Member", viewer: "Viewer", billing: "Billing" };

export function PermissionsMatrixTable({ matrix }: { matrix: Capability[] }) {
  const groups = new Map<string, Capability[]>();
  for (const capability of matrix) {
    const group = groups.get(capability.feature_area) ?? [];
    group.push(capability);
    groups.set(capability.feature_area, group);
  }

  return (
    <div className="overflow-x-auto rounded-lg border border-border-strong">
      <Table>
        <TableHeader>
          <TableRow>
            <TableHead>Feature / Action</TableHead>
            {ROLE_ORDER.map((r) => (
              <TableHead key={r} className="text-center">
                {ROLE_LABELS[r]}
              </TableHead>
            ))}
          </TableRow>
        </TableHeader>
        <TableBody>
          {[...groups.entries()].map(([area, capabilities]) => (
            <Fragment key={area}>
              <TableRow className="bg-panel/60">
                <TableCell colSpan={ROLE_ORDER.length + 1} className="text-mono-code uppercase tracking-wide text-ink-500">
                  {area}
                </TableCell>
              </TableRow>
              {capabilities.map((c) => (
                <TableRow key={`${area}-${c.label}`}>
                  <TableCell className="text-body text-ink-300">{c.label}</TableCell>
                  {ROLE_ORDER.map((r) => (
                    <TableCell key={r} className="text-center">
                      {c.allowed_roles.includes(r) ? <Check className="mx-auto size-3.5 text-health-healthy" /> : <X className="mx-auto size-3.5 text-ink-500/50" />}
                    </TableCell>
                  ))}
                </TableRow>
              ))}
            </Fragment>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}
