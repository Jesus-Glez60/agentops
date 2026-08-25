"use client";

import { useState, type FormEvent } from "react";
import { useRouter } from "next/navigation";
import { toast } from "sonner";
import { Check } from "lucide-react";
import { signupWithPassword, AuthClientError } from "@/lib/auth/client";
import { PASSWORD_REQUIREMENTS, validateConfirmPassword, validateEmail, validateName } from "@/lib/auth/validation";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { PasswordInput } from "@/components/auth/password-input";
import { PasswordRequirementsList } from "@/components/auth/password-requirements";

// UX convention for this app (see .agentops/notes): password-field feedback
// (the requirements checklist, the confirm-password match state) is
// line-level/inline and live -- it's the thing the user is actively typing
// and needs to see change as they type. Everything else (name, email) is a
// toast on submit attempt instead of per-field red text, since those are
// simple presence/shape checks a user fixes once, not something they need
// to watch update character-by-character.
export function SignupForm({ redirectTo, inviteToken }: { redirectTo: string; inviteToken?: string }) {
  const router = useRouter();
  const [firstName, setFirstName] = useState("");
  const [lastName, setLastName] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirmPassword, setConfirmPassword] = useState("");
  const [confirmTouched, setConfirmTouched] = useState(false);
  const [attemptedSubmit, setAttemptedSubmit] = useState(false);
  const [pending, setPending] = useState(false);

  const firstNameError = validateName(firstName, "First name");
  const lastNameError = validateName(lastName, "Last name");
  const emailError = validateEmail(email);
  const passwordValid = PASSWORD_REQUIREMENTS.every((r) => r.test(password));
  const confirmError = validateConfirmPassword(password, confirmPassword);
  const showConfirmError = (confirmTouched || attemptedSubmit) && !!confirmError;
  const showConfirmMatch = (confirmTouched || attemptedSubmit) && !confirmError;

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setAttemptedSubmit(true);

    const otherFieldMessages = [firstNameError, lastNameError, emailError].filter((m): m is string => !!m);
    if (otherFieldMessages.length > 0) {
      toast.error(otherFieldMessages.length === 1 ? otherFieldMessages[0] : "Please fix the highlighted fields.", {
        description: otherFieldMessages.length > 1 ? otherFieldMessages.join(" ") : undefined,
      });
    }

    if (otherFieldMessages.length > 0 || !passwordValid || !!confirmError) return;

    setPending(true);
    try {
      if (inviteToken) {
        await signupWithPassword(firstName.trim(), lastName.trim(), email, password, inviteToken);
      } else {
        await signupWithPassword(firstName.trim(), lastName.trim(), email, password);
      }
      router.push(redirectTo);
      router.refresh();
    } catch (err) {
      const message = err instanceof AuthClientError ? err.message : "Something went wrong. Please try again.";
      toast.error(message);
    } finally {
      setPending(false);
    }
  }

  return (
    <form onSubmit={handleSubmit} className="space-y-4" noValidate>
      <div className="grid grid-cols-2 gap-3">
        <div className="space-y-1.5">
          <label htmlFor="signup-first-name" className="text-section text-ink-300">
            First name
          </label>
          <Input id="signup-first-name" autoComplete="given-name" value={firstName} onChange={(e) => setFirstName(e.target.value)} disabled={pending} aria-invalid={attemptedSubmit && !!firstNameError} />
        </div>
        <div className="space-y-1.5">
          <label htmlFor="signup-last-name" className="text-section text-ink-300">
            Last name
          </label>
          <Input id="signup-last-name" autoComplete="family-name" value={lastName} onChange={(e) => setLastName(e.target.value)} disabled={pending} aria-invalid={attemptedSubmit && !!lastNameError} />
        </div>
      </div>
      <div className="space-y-1.5">
        <label htmlFor="signup-email" className="text-section text-ink-300">
          Email
        </label>
        <Input id="signup-email" type="email" autoComplete="email" value={email} onChange={(e) => setEmail(e.target.value)} disabled={pending} aria-invalid={attemptedSubmit && !!emailError} />
      </div>
      <div className="space-y-1.5">
        <label htmlFor="signup-password" className="text-section text-ink-300">
          Password
        </label>
        <PasswordInput id="signup-password" autoComplete="new-password" value={password} onChange={setPassword} disabled={pending} ariaInvalid={attemptedSubmit && !passwordValid} />
        <PasswordRequirementsList password={password} showUnmetAsError={attemptedSubmit} />
      </div>
      <div className="space-y-1.5">
        <label htmlFor="signup-confirm-password" className="text-section text-ink-300">
          Confirm password
        </label>
        <PasswordInput
          id="signup-confirm-password"
          autoComplete="new-password"
          value={confirmPassword}
          onChange={setConfirmPassword}
          onBlur={() => setConfirmTouched(true)}
          disabled={pending}
          ariaInvalid={showConfirmError}
        />
        {showConfirmError && <p className="text-body text-destructive">{confirmError}</p>}
        {showConfirmMatch && (
          <p className="flex items-center gap-1.5 text-body text-health-healthy">
            <Check className="size-3.5" /> Passwords match
          </p>
        )}
      </div>
      <Button type="submit" className="w-full" disabled={pending}>
        {pending ? "Signing up…" : "Sign up"}
      </Button>
    </form>
  );
}
