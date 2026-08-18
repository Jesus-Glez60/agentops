"use client";

import { useState } from "react";
import { useSWRConfig } from "swr";
import { toast } from "sonner";
import { ShieldCheck, Check } from "lucide-react";
import { begin2fa, confirm2fa, disable2fa, regenerateBackupCodes, PROFILE_SWR_KEY, type TwoFactorEnrollment } from "@/lib/api/profile-api";
import type { SessionUser } from "@/lib/auth/types";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from "@/components/ui/dialog";

export function TwoFactorCard({ user }: { user: SessionUser }) {
  const { mutate } = useSWRConfig();
  const [enrollOpen, setEnrollOpen] = useState(false);
  const [disableOpen, setDisableOpen] = useState(false);
  const [regenerateOpen, setRegenerateOpen] = useState(false);

  return (
    <Card>
      <CardHeader className="flex-row items-center justify-between border-b border-border-strong pb-4">
        <CardTitle className="flex items-center gap-2">
          <ShieldCheck className="size-4 text-ink-400" />
          Two-Factor Authentication
        </CardTitle>
        {user.two_factor_enabled ? (
          <Badge variant="outline" className="border-health-healthy/40 text-health-healthy">
            <Check className="size-3" /> Enabled
          </Badge>
        ) : (
          <Badge variant="outline">Not enabled</Badge>
        )}
      </CardHeader>
      <CardContent className="flex items-center justify-between px-6 py-4">
        <p className="max-w-md text-body text-ink-400">
          {user.two_factor_enabled ? "Your account requires a code from your authenticator app at login, in addition to your password." : "Add an authenticator app as a second factor at login."}
        </p>
        <div className="flex shrink-0 gap-2">
          {user.two_factor_enabled ? (
            <>
              <Button variant="outline" size="sm" onClick={() => setRegenerateOpen(true)}>
                Regenerate backup codes
              </Button>
              <Button variant="outline" size="sm" onClick={() => setDisableOpen(true)}>
                Remove
              </Button>
            </>
          ) : (
            <Button size="sm" onClick={() => setEnrollOpen(true)}>
              Enable 2FA
            </Button>
          )}
        </div>
      </CardContent>
      <EnrollDialog open={enrollOpen} onOpenChange={setEnrollOpen} onEnabled={() => mutate(PROFILE_SWR_KEY)} />
      <PasswordGatedBackupCodesDialog
        open={regenerateOpen}
        onOpenChange={setRegenerateOpen}
        title="Regenerate backup codes"
        description="Your existing backup codes will stop working. Enter your password to continue."
        action={regenerateBackupCodes}
      />
      <DisableDialog open={disableOpen} onOpenChange={setDisableOpen} onDisabled={() => mutate(PROFILE_SWR_KEY)} />
    </Card>
  );
}

function EnrollDialog({ open, onOpenChange, onEnabled }: { open: boolean; onOpenChange: (open: boolean) => void; onEnabled: () => void }) {
  const [enrollment, setEnrollment] = useState<TwoFactorEnrollment | null>(null);
  const [code, setCode] = useState("");
  const [backupCodes, setBackupCodes] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);

  function reset() {
    setEnrollment(null);
    setCode("");
    setBackupCodes(null);
  }

  async function handleOpenChange(next: boolean) {
    onOpenChange(next);
    if (next && !enrollment) {
      try {
        setEnrollment(await begin2fa());
      } catch (err) {
        toast.error(err instanceof Error ? err.message : "Couldn't start 2FA setup. Please try again.");
        onOpenChange(false);
      }
    }
    if (!next) reset();
  }

  async function handleConfirm(e: React.FormEvent) {
    e.preventDefault();
    if (!code.trim()) return;
    setPending(true);
    try {
      const { backup_codes } = await confirm2fa(code.trim());
      setBackupCodes(backup_codes);
      onEnabled();
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "That code didn't work. Please try again.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        {backupCodes ? (
          <>
            <DialogHeader>
              <DialogTitle>Save your backup codes</DialogTitle>
              <DialogDescription>Each code can be used once if you lose access to your authenticator app. Store them somewhere safe — they won&apos;t be shown again.</DialogDescription>
            </DialogHeader>
            <div className="grid grid-cols-2 gap-2 rounded-md border border-border-strong bg-panel p-4 font-mono text-body">
              {backupCodes.map((c) => (
                <span key={c}>{c}</span>
              ))}
            </div>
            <DialogFooter>
              <Button size="sm" onClick={() => handleOpenChange(false)}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : enrollment ? (
          <form onSubmit={handleConfirm}>
            <DialogHeader>
              <DialogTitle>Scan this QR code</DialogTitle>
              <DialogDescription>Scan with your authenticator app (Google Authenticator, 1Password, etc.), then enter the 6-digit code it shows.</DialogDescription>
            </DialogHeader>
            <div className="flex flex-col items-center gap-3 py-4">
              {/* eslint-disable-next-line @next/next/no-img-element -- data: URI, not an optimizable remote image */}
              <img src={enrollment.qr_data_uri} alt="2FA setup QR code" className="size-40 rounded-md border border-border-strong" />
              <p className="text-mono-code text-ink-500">Can&apos;t scan? Enter manually: {enrollment.secret}</p>
              <Input inputMode="numeric" placeholder="123456" value={code} onChange={(e) => setCode(e.target.value)} autoFocus className="w-40 text-center" />
            </div>
            <DialogFooter>
              <Button type="submit" size="sm" disabled={pending || !code.trim()}>
                {pending ? "Verifying…" : "Verify and enable"}
              </Button>
            </DialogFooter>
          </form>
        ) : (
          <p className="py-8 text-center text-body text-ink-500">Setting up…</p>
        )}
      </DialogContent>
    </Dialog>
  );
}

function DisableDialog({ open, onOpenChange, onDisabled }: { open: boolean; onOpenChange: (open: boolean) => void; onDisabled: () => void }) {
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);

  function handleOpenChange(next: boolean) {
    onOpenChange(next);
    if (!next) setPassword("");
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setPending(true);
    try {
      await disable2fa(password);
      onDisabled();
      toast.success("Two-factor authentication removed");
      handleOpenChange(false);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't verify your password. Please try again.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>Remove two-factor authentication</DialogTitle>
            <DialogDescription>Enter your password to confirm.</DialogDescription>
          </DialogHeader>
          <div className="py-4">
            <Input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="••••••••" autoFocus required />
          </div>
          <DialogFooter>
            <Button type="submit" variant="destructive" size="sm" disabled={pending || !password}>
              {pending ? "Removing…" : "Remove 2FA"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

function PasswordGatedBackupCodesDialog({
  open,
  onOpenChange,
  title,
  description,
  action,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  action: (password: string) => Promise<{ backup_codes: string[] }>;
}) {
  const [password, setPassword] = useState("");
  const [backupCodes, setBackupCodes] = useState<string[] | null>(null);
  const [pending, setPending] = useState(false);

  function handleOpenChange(next: boolean) {
    onOpenChange(next);
    if (!next) {
      setPassword("");
      setBackupCodes(null);
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    setPending(true);
    try {
      const { backup_codes } = await action(password);
      setBackupCodes(backup_codes);
    } catch (err) {
      toast.error(err instanceof Error ? err.message : "Couldn't verify your password. Please try again.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        {backupCodes ? (
          <>
            <DialogHeader>
              <DialogTitle>New backup codes</DialogTitle>
              <DialogDescription>Your old codes no longer work. Save these somewhere safe — they won&apos;t be shown again.</DialogDescription>
            </DialogHeader>
            <div className="grid grid-cols-2 gap-2 rounded-md border border-border-strong bg-panel p-4 font-mono text-body">
              {backupCodes.map((c) => (
                <span key={c}>{c}</span>
              ))}
            </div>
            <DialogFooter>
              <Button size="sm" onClick={() => handleOpenChange(false)}>
                Done
              </Button>
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>{title}</DialogTitle>
              <DialogDescription>{description}</DialogDescription>
            </DialogHeader>
            <div className="py-4">
              <Input type="password" autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} placeholder="••••••••" autoFocus required />
            </div>
            <DialogFooter>
              <Button type="submit" size="sm" disabled={pending || !password}>
                {pending ? "Verifying…" : "Continue"}
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogContent>
    </Dialog>
  );
}
