"use client";

// Literal 3-step wizard (redesign plan Phase 2), reversing the prior
// checklist-not-wizard decision now that full design parity is the explicit
// goal -- see the project note recorded alongside this change for the
// original checklist rationale and why it's superseded. The thing that
// actually preserves that rationale's "never block the escape hatch"
// finding: the left-panel step list is directly clickable (jump to any
// step, not just Next/Back), and "Skip setup" is always one click away --
// this is wizard-*shaped*, not wizard-*gated*.
import { useState } from "react";
import { useRouter } from "next/navigation";
import useSWR from "swr";
import { toast } from "sonner";
import { Check, ChevronRight, Cloud, FolderSearch, Key } from "lucide-react";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { SessionUser } from "@/lib/auth/types";
import { getTeam, renameOrg, TEAM_SWR_KEY } from "@/lib/api/team-api";
import { getRepos, REPOS_SWR_KEY } from "@/lib/api/repos-api";
import { completeOnboarding } from "@/lib/api/profile-api";
import { InviteMemberDialog } from "@/components/team/invite-member-dialog";
import { CopyButton } from "@/components/shared/copy-button";
import { ToolSelect, DEFAULT_SELECTED_AGENTS } from "@/components/onboarding/tool-select";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AuthSplitLayout, GradientText } from "@/components/auth/auth-split-layout";
import { cn } from "@/lib/utils";

const STEP_COUNT = 3;

interface StepMeta {
  title: string;
  description: string;
}

const STEPS: StepMeta[] = [
  { title: "Who's this workspace for?", description: "Solo, or setting up a team." },
  { title: "Connect your coding tool", description: "Register agentops's MCP server with your agent." },
  { title: "Add your first repository", description: "Optional — an agent can also register one for you." },
];

export function WelcomeWizard({ user, apiUrl, apiUrlIsGuessed }: { user: SessionUser; apiUrl: string; apiUrlIsGuessed: boolean }) {
  const router = useRouter();
  const { data: team } = useSWR(TEAM_SWR_KEY, getTeam); // also triggers the ensure_membership Owner backfill as a side effect
  const { data: repos } = useSWR(REPOS_SWR_KEY, getRepos);

  const [step, setStep] = useState(0);
  const [finishing, setFinishing] = useState(false);

  const [workspaceDone, setWorkspaceDone] = useState(false);
  const [workspaceMode, setWorkspaceMode] = useState<"solo" | "team" | null>(null);
  const [orgName, setOrgName] = useState("");
  const [savingOrg, setSavingOrg] = useState(false);

  const [connectDone, setConnectDone] = useState(false);
  // See onboarding-checklist's original doc comment on this field (now
  // superseded, but the underlying reasoning carries over unchanged): a
  // solo dev can still self-host on a separate box, so this stays an
  // explicit, always-overridable choice, only *hinted* by team size.
  const [connectMode, setConnectMode] = useState<"local" | "remote" | null>(null);
  const effectiveConnectMode = connectMode ?? (team && team.member_count > 1 ? "remote" : "local");
  const [selectedAgents, setSelectedAgents] = useState<string[]>(DEFAULT_SELECTED_AGENTS);
  const agentsArg = selectedAgents.length > 0 ? selectedAgents.join(",") : DEFAULT_SELECTED_AGENTS.join(",");
  const localCommand = `npx agentops-cli connect --agents ${agentsArg}`;
  const connectNpxCommand = `npx agentops-cli connect --remote ${apiUrl} --agents ${agentsArg}`;
  const connectScriptCommand = `curl -fsSL ${apiUrl}/connect.sh?agents=${agentsArg} | sh`;

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

  async function finish() {
    setFinishing(true);
    try {
      await completeOnboarding();
    } catch {
      // Non-fatal -- if this fails, (app)/layout.tsx just sends them right
      // back here next load.
    } finally {
      router.push("/");
      router.refresh();
    }
  }

  // Same reasoning as the retired checklist's `navigateWithinApp`: any
  // `(app)/*` page redirects back here if onboarding isn't marked complete,
  // so stepping out to go set up a real repository needs to mark it done
  // first or the user bounces right back to a reset wizard.
  async function navigateWithinApp(href: string) {
    try {
      await completeOnboarding();
    } catch {
      // Non-fatal, same reasoning as finish().
    }
    router.push(href);
  }

  const doneFlags = [workspaceDone, connectDone, false];

  return (
    <AuthSplitLayout
      eyebrow="Account created ✓"
      eyebrowColor="var(--mauve)"
      headline={
        <>
          Welcome, {user.first_name}. Let&apos;s give your agents{" "}
          <GradientText stops={["#cba6f7", "#f5c2e7", "#fab387"]}>a map.</GradientText>
        </>
      }
      subtitle="A few quick steps to get your agents reading real project knowledge instead of starting from zero every session."
      leftExtra={
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-2">
            {STEPS.map((s, i) => (
              <button
                key={s.title}
                type="button"
                onClick={() => setStep(i)}
                className={cn(
                  "flex items-center gap-3 rounded-lg border px-3 py-2.5 text-left transition-colors",
                  step === i ? "border-primary bg-raised" : "border-border-strong bg-panel hover:border-border-strong/80",
                )}
              >
                <span
                  className={cn(
                    "flex size-5 shrink-0 items-center justify-center rounded-full border text-[11px] font-bold",
                    doneFlags[i] ? "border-health-healthy bg-health-healthy/20 text-health-healthy" : step === i ? "border-primary text-primary" : "border-border-strong text-ink-500",
                  )}
                >
                  {doneFlags[i] ? <Check className="size-3" /> : i + 1}
                </span>
                <span className="flex flex-col">
                  <span className="text-body font-medium text-ink-100">{s.title}</span>
                  <span className="text-body text-ink-500">{s.description}</span>
                </span>
              </button>
            ))}
          </div>
          <button type="button" onClick={finish} disabled={finishing} className="self-start text-body text-ink-500 underline underline-offset-2 hover:text-ink-300">
            {finishing ? "Continuing…" : "Skip setup and go to the dashboard"}
          </button>
        </div>
      }
    >
      <div className="flex flex-col gap-5 rounded-2xl border border-border-strong bg-panel p-6">
        <div className="flex items-center justify-between">
          <h2 className="text-display-card font-bold text-ink-100">{STEPS[step].title}</h2>
          <span className="font-mono text-[12px] text-ink-500">
            Step {step + 1} of {STEP_COUNT}
          </span>
        </div>

        {step === 0 && (
          <div className="flex flex-col gap-3">
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
          </div>
        )}

        {step === 1 && (
          <div className="flex flex-col gap-3">
            <div className="flex gap-2">
              <Button size="sm" variant={effectiveConnectMode === "local" ? "default" : "outline"} onClick={() => setConnectMode("local")}>
                This device
              </Button>
              <Button size="sm" variant={effectiveConnectMode === "remote" ? "default" : "outline"} onClick={() => setConnectMode("remote")}>
                A separate server
              </Button>
            </div>
            <ToolSelect selected={selectedAgents} onChange={setSelectedAgents} />
            {effectiveConnectMode === "local" ? (
              <>
                <p className="text-body text-ink-400">From your own machine, in your project repo, run:</p>
                <div className="flex items-center gap-2">
                  <code className="flex-1 truncate rounded-md border border-border-strong bg-raised px-3 py-2 text-mono-code text-ink-200">{localCommand}</code>
                  <CopyButton value={localCommand} />
                </div>
              </>
            ) : (
              <>
                {apiUrlIsGuessed && (
                  <p className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-body text-amber-500">
                    Couldn&apos;t confirm this server&apos;s public API address — guessed <code className="text-mono-code">{apiUrl}</code>.
                  </p>
                )}
                <p className="text-body text-ink-400">From your own machine (installs the CLI if it isn&apos;t already there, then opens a browser to log in), run:</p>
                <div className="flex items-center gap-2">
                  <code className="flex-1 truncate rounded-md border border-border-strong bg-raised px-3 py-2 text-mono-code text-ink-200">{connectNpxCommand}</code>
                  <CopyButton value={connectNpxCommand} />
                </div>
                <Collapsible>
                  <CollapsibleTrigger className="group flex items-center gap-1 text-body text-ink-500 hover:text-ink-300">
                    <ChevronRight className="size-3.5 transition-transform group-data-[state=open]:rotate-90" />
                    Advanced: install via curl instead
                  </CollapsibleTrigger>
                  <CollapsibleContent className="pt-2">
                    <div className="flex items-center gap-2">
                      <code className="flex-1 truncate rounded-md border border-border-strong bg-raised px-3 py-2 text-mono-code text-ink-200">{connectScriptCommand}</code>
                      <CopyButton value={connectScriptCommand} />
                    </div>
                  </CollapsibleContent>
                </Collapsible>
              </>
            )}
            <label className="flex items-center gap-2 text-body text-ink-300">
              <input type="checkbox" checked={connectDone} onChange={(e) => setConnectDone(e.target.checked)} className="size-4 rounded border-border-strong" />
              I&apos;ve done this
            </label>
          </div>
        )}

        {step === 2 && (
          <div className="flex flex-col gap-2">
            {repos && repos.connections.length > 0 ? (
              <p className="text-body text-ink-400">
                <span className="font-medium text-ink-100">{repos.connections.length}</span> repositor{repos.connections.length === 1 ? "y" : "ies"} already connected.
              </p>
            ) : (
              <p className="text-body text-ink-500">An agent can also register a repository automatically the first time it works in it — this step is entirely optional.</p>
            )}
            <RepoOptionRow icon={Cloud} label="Connect via GitHub App" onClick={() => navigateWithinApp("/repositories/connect")} />
            <RepoOptionRow icon={Key} label="Connect over SSH" onClick={() => navigateWithinApp("/repositories/connect/ssh")} />
            <RepoOptionRow icon={FolderSearch} label="Connect a local path" onClick={() => navigateWithinApp("/repositories/connect/local")} />
          </div>
        )}

        <div className="flex items-center justify-between pt-2">
          <Button variant="outline" size="sm" disabled={step === 0} onClick={() => setStep((s) => Math.max(0, s - 1))}>
            Back
          </Button>
          {step < STEP_COUNT - 1 ? (
            <Button size="sm" onClick={() => setStep((s) => Math.min(STEP_COUNT - 1, s + 1))}>
              Next
            </Button>
          ) : (
            <Button size="sm" disabled={finishing} onClick={finish}>
              {finishing ? "Continuing…" : "Continue to dashboard"}
            </Button>
          )}
        </div>
      </div>
    </AuthSplitLayout>
  );
}

function RepoOptionRow({ icon: Icon, label, onClick }: { icon: typeof Cloud; label: string; onClick: () => void }) {
  return (
    <button type="button" onClick={onClick} className="flex items-center gap-3 rounded-md border border-border-strong bg-raised px-3 py-2.5 text-left transition-colors hover:border-border-strong/80">
      <Icon className="size-4 shrink-0 text-ink-500" />
      <span className="text-body font-medium text-ink-100">{label}</span>
      <ChevronRight className="ml-auto size-4 text-ink-500" />
    </button>
  );
}
