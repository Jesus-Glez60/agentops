"use client";

// A checklist, not a forced linear wizard -- items are independently
// completable in any order, and "Continue to dashboard" is always
// clickable regardless of how many items are done. Both choices are
// deliberate: checklists outperform gated wizards for a handful of
// unordered optional setup tasks, and never blocking the escape hatch is
// the single most-repeated finding across onboarding UX research (see the
// onboarding plan's UX research section for sources). The only thing this
// page is actually mandatory for is being *shown* once -- that's enforced
// by (app)/layout.tsx's redirect, not by anything in here.
import { useState } from "react";
import { useRouter } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { Check, ChevronDown, ChevronRight } from "lucide-react";
import type { SessionUser } from "@/lib/auth/types";
import { getTeam, renameOrg, TEAM_SWR_KEY } from "@/lib/api/team-api";
import { updateProfile, completeOnboarding } from "@/lib/api/profile-api";
import { InviteMemberDialog } from "@/components/team/invite-member-dialog";
import { CopyButton } from "@/components/shared/copy-button";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { Checkbox } from "@/components/ui/checkbox";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";

const CONNECT_COMMAND = "agentops connect";

function ChecklistItem({ title, done, expanded, onToggle, children }: { title: string; done: boolean; expanded: boolean; onToggle: () => void; children: React.ReactNode }) {
  return (
    <div className="rounded-lg border border-border-strong">
      <button type="button" onClick={onToggle} className="flex w-full items-center gap-3 px-4 py-3 text-left">
        <span className={`flex size-5 shrink-0 items-center justify-center rounded-full border ${done ? "border-health-healthy bg-health-healthy/20 text-health-healthy" : "border-border-strong text-ink-500"}`}>
          {done ? <Check className="size-3.5" /> : null}
        </span>
        <span className="flex-1 text-body font-medium text-ink-100">{title}</span>
        {expanded ? <ChevronDown className="size-4 text-ink-500" /> : <ChevronRight className="size-4 text-ink-500" />}
      </button>
      {expanded && <div className="border-t border-border-strong px-4 py-4">{children}</div>}
    </div>
  );
}

export function OnboardingChecklist({ user, origin }: { user: SessionUser; origin: string }) {
  const router = useRouter();
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam); // also triggers the ensure_membership Owner backfill as a side effect
  const [expanded, setExpanded] = useState<string | null>("workspace");
  const [finishing, setFinishing] = useState(false);

  const [workspaceDone, setWorkspaceDone] = useState(false);
  const [workspaceMode, setWorkspaceMode] = useState<"solo" | "team" | null>(null);
  const [orgName, setOrgName] = useState("");
  const [savingOrg, setSavingOrg] = useState(false);

  const [profileDone, setProfileDone] = useState(false);
  const [handle, setHandle] = useState(user.handle ?? "");
  const [location, setLocation] = useState(user.location);
  const [bio, setBio] = useState(user.bio);
  const [savingProfile, setSavingProfile] = useState(false);

  const [connectDone, setConnectDone] = useState(false);
  // Whether agentops runs on this device or on a separate server isn't
  // knowable from team size alone -- a solo dev can self-host on their own
  // personal server too, in which case they're still "solo" but still
  // need the --remote form. So this is an explicit, always-overridable
  // choice, only *hinted* by team size once it's loaded (a real team
  // almost always implies shared hosting), never inferred silently.
  const [connectMode, setConnectMode] = useState<"local" | "remote" | null>(null);
  const effectiveConnectMode = connectMode ?? (team && team.member_count > 1 ? "remote" : "local");

  function toggle(item: string) {
    setExpanded((cur) => (cur === item ? null : item));
  }

  async function saveSolo() {
    setSavingOrg(true);
    try {
      await renameOrg(`${user.first_name}'s workspace`);
      setWorkspaceMode("solo");
      setWorkspaceDone(true);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't set up your workspace. Please try again.");
    } finally {
      setSavingOrg(false);
    }
  }

  async function saveTeamName() {
    if (!orgName.trim()) return;
    setSavingOrg(true);
    try {
      await renameOrg(orgName.trim());
      setWorkspaceDone(true);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't save that name. Please try again.");
    } finally {
      setSavingOrg(false);
    }
  }

  async function saveProfile() {
    setSavingProfile(true);
    try {
      await updateProfile({ handle: handle.trim(), location: location.trim(), bio: bio.trim() });
      setProfileDone(true);
      toast.success("Profile updated");
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't update your profile. Please try again.");
    } finally {
      setSavingProfile(false);
    }
  }

  async function finish() {
    setFinishing(true);
    try {
      await completeOnboarding();
    } catch {
      // Non-fatal -- if this fails, (app)/layout.tsx just sends them right
      // back here next load. Don't trap the user behind a network hiccup.
    } finally {
      router.push("/");
      router.refresh();
    }
  }

  return (
    <main className="flex min-h-screen items-center justify-center bg-canvas p-4">
      <Card className="w-full max-w-lg">
        <CardHeader>
          <CardTitle className="text-page-title">Welcome to AgentOps</CardTitle>
          <CardDescription>A few optional things to get set up — skip anything, any time.</CardDescription>
        </CardHeader>
        <CardContent className="space-y-3">
          <div className="flex items-center gap-3 rounded-lg border border-health-healthy/30 bg-health-healthy/10 px-4 py-3">
            <span className="flex size-5 shrink-0 items-center justify-center rounded-full border border-health-healthy bg-health-healthy/20 text-health-healthy">
              <Check className="size-3.5" />
            </span>
            <span className="text-body font-medium text-ink-100">Account created</span>
          </div>

          {team?.is_owner && (
            <ChecklistItem title="Set up your workspace" done={workspaceDone} expanded={expanded === "workspace"} onToggle={() => toggle("workspace")}>
              {workspaceMode === null ? (
                <div className="flex gap-2">
                  <Button size="sm" variant="outline" disabled={savingOrg} onClick={saveSolo}>
                    I&apos;m working solo
                  </Button>
                  <Button size="sm" variant="outline" onClick={() => setWorkspaceMode("team")}>
                    I&apos;m setting up a team
                  </Button>
                </div>
              ) : workspaceMode === "solo" ? (
                <p className="text-body text-ink-400">You&apos;re all set — working solo in {`${user.first_name}'s workspace`}.</p>
              ) : (
                <div className="space-y-3">
                  <div className="flex gap-2">
                    <Input value={orgName} onChange={(e) => setOrgName(e.target.value)} placeholder="Your organization's name" disabled={savingOrg} />
                    <Button size="sm" disabled={savingOrg || !orgName.trim()} onClick={saveTeamName}>
                      {savingOrg ? "Saving…" : "Save"}
                    </Button>
                  </div>
                  <InviteMemberDialog />
                </div>
              )}
            </ChecklistItem>
          )}

          <ChecklistItem title="Complete your profile" done={profileDone} expanded={expanded === "profile"} onToggle={() => toggle("profile")}>
            <div className="space-y-3">
              <Input value={handle} onChange={(e) => setHandle(e.target.value)} placeholder="Handle" disabled={savingProfile} />
              <Input value={location} onChange={(e) => setLocation(e.target.value)} placeholder="Location" disabled={savingProfile} />
              <Textarea value={bio} onChange={(e) => setBio(e.target.value)} placeholder="A short bio" rows={2} disabled={savingProfile} />
              <Button size="sm" disabled={savingProfile} onClick={saveProfile}>
                {savingProfile ? "Saving…" : "Save"}
              </Button>
            </div>
          </ChecklistItem>

          <ChecklistItem title="Connect your coding tool" done={connectDone} expanded={expanded === "connect"} onToggle={() => toggle("connect")}>
            <div className="space-y-3">
              <div className="flex gap-2">
                <Button size="sm" variant={effectiveConnectMode === "local" ? "default" : "outline"} onClick={() => setConnectMode("local")}>
                  This device
                </Button>
                <Button size="sm" variant={effectiveConnectMode === "remote" ? "default" : "outline"} onClick={() => setConnectMode("remote")}>
                  A separate server
                </Button>
              </div>
              {effectiveConnectMode === "local" ? (
                <>
                  <p className="text-body text-ink-400">From your own machine, in your project repo, run:</p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{CONNECT_COMMAND}</code>
                    <CopyButton value={CONNECT_COMMAND} />
                  </div>
                </>
              ) : (
                <>
                  <p className="text-body text-ink-400">
                    Generate a personal API key from <span className="font-medium text-ink-200">Settings → API Keys</span>, then from your own machine, in your project repo, run:
                  </p>
                  <div className="flex items-center gap-2">
                    <code className="flex-1 truncate rounded-md border border-border-strong bg-panel px-3 py-2 text-mono-code text-ink-200">{`${CONNECT_COMMAND} --remote ${origin}`}</code>
                    <CopyButton value={`${CONNECT_COMMAND} --remote ${origin}`} />
                  </div>
                </>
              )}
              <p className="text-body text-ink-500">Registers agentops&apos;s MCP server and distributes instructions to Claude Code, Cursor, Codex CLI, Gemini CLI, or another tool you choose.</p>
              <div className="flex items-center gap-2">
                <Checkbox id="connect-done" checked={connectDone} onCheckedChange={(v) => setConnectDone(v === true)} />
                <label htmlFor="connect-done" className="text-body text-ink-300">
                  I&apos;ve done this
                </label>
              </div>
            </div>
          </ChecklistItem>
        </CardContent>
        <CardContent className="pt-0">
          <Button className="w-full" disabled={finishing} onClick={finish}>
            {finishing ? "Continuing…" : "Continue to dashboard"}
          </Button>
        </CardContent>
      </Card>
    </main>
  );
}
