"use client";

import { useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { loginWithPassword, AuthClientError } from "@/lib/auth/client";
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

  const emailError = validateEmail(email);
  const passwordError = validateRequired(password, "Password");

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setAttemptedSubmit(true);

    if (emailError) toast.error(emailError);
    if (emailError || passwordError) return;

    setPending(true);
    try {
      await loginWithPassword(email, password);
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
