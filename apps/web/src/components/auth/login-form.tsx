"use client";

import { useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { loginWithPassword, completeLogin2fa, AuthClientError } from "@/lib/auth/client";
import { validateEmail, validateRequired } from "@/lib/auth/validation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/auth/password-input";

// Same convention as SignupForm: password feedback is inline/line-level,
// everything else (just email here) is a toast on submit attempt.
export function LoginForm({ redirectTo }: { redirectTo: string }) {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [attemptedSubmit, setAttemptedSubmit] = useState(false);
  const [pending, setPending] = useState(false);
  const [challengeToken, setChallengeToken] = useState<string | null>(null);
  const [code, setCode] = useState("");

  const emailError = validateEmail(email);
  const passwordError = validateRequired(password, "Password");

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setAttemptedSubmit(true);

    if (emailError) toast.error(emailError);
    if (emailError || passwordError) return;

    setPending(true);
    try {
      const result = await loginWithPassword(email, password);
      if ("two_factor_required" in result) {
        setChallengeToken(result.challenge_token);
        return;
      }
      router.push(redirectTo);
      router.refresh();
    } catch (err) {
      // The backend deliberately returns the same generic message for "no
      // such email" and "wrong password" -- passed straight through as one
      // toast rather than re-attributed to a specific field, so this UI
      // doesn't leak which one was wrong.
      const message = err instanceof AuthClientError ? err.message : "Something went wrong. Please try again.";
      toast.error(message);
    } finally {
      setPending(false);
    }
  }

  async function handleSubmit2fa(e: FormEvent) {
    e.preventDefault();
    if (!challengeToken || !code.trim()) return;
    setPending(true);
    try {
      await completeLogin2fa(challengeToken, code.trim());
      router.push(redirectTo);
      router.refresh();
    } catch (err) {
      const message = err instanceof AuthClientError ? err.message : "Something went wrong. Please try again.";
      toast.error(message);
    } finally {
      setPending(false);
    }
  }

  if (challengeToken) {
    return (
      <form onSubmit={handleSubmit2fa} className="space-y-4" noValidate>
        <div className="space-y-1.5">
          <label htmlFor="login-2fa-code" className="text-section text-ink-300">
            Two-factor code
          </label>
          <Input id="login-2fa-code" inputMode="numeric" autoComplete="one-time-code" placeholder="123456" value={code} onChange={(e) => setCode(e.target.value)} disabled={pending} autoFocus />
          <p className="text-body text-ink-500">Enter the code from your authenticator app, or a backup code.</p>
        </div>
        <Button type="submit" className="w-full" disabled={pending || !code.trim()}>
          {pending ? "Verifying…" : "Verify"}
        </Button>
      </form>
    );
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4" noValidate>
      <div className="space-y-1.5">
        <label htmlFor="login-email" className="text-section text-ink-300">
          Email
        </label>
        <Input id="login-email" type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} disabled={pending} aria-invalid={attemptedSubmit && !!emailError} />
      </div>
      <div className="space-y-1.5">
        <label htmlFor="login-password" className="text-section text-ink-300">
          Password
        </label>
        <PasswordInput id="login-password" autoComplete="current-password" value={password} onChange={setPassword} disabled={pending} ariaInvalid={attemptedSubmit && !!passwordError} />
        {attemptedSubmit && passwordError && <p className="text-body text-destructive">{passwordError}</p>}
      </div>
      <Button type="submit" className="w-full" disabled={pending}>
        {pending ? "Logging in…" : "Log in"}
      </Button>
    </form>
  );
}
