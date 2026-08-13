// Shared by both the login and signup forms so their validation can't drift.
export function validateEmail(email: string): string | null {
  if (!email.trim()) return "Email is required.";
  if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) return "Enter a valid email address.";
  return null;
}

/** Login only -- just "did you type something," not a complexity check. An
 * existing password's shape was already decided at signup time; re-running
 * signup's rules against a login attempt would produce a misleading error
 * message when the real failure is going to be "invalid email or password"
 * from the server anyway. */
export function validateRequired(value: string, fieldLabel: string): string | null {
  if (!value) return `${fieldLabel} is required.`;
  return null;
}

export function validateConfirmPassword(password: string, confirmPassword: string): string | null {
  if (!confirmPassword) return "Please confirm your password.";
  if (password !== confirmPassword) return "Passwords do not match.";
  return null;
}

export function validateName(name: string, fieldLabel: string): string | null {
  if (!name.trim()) return `${fieldLabel} is required.`;
  return null;
}

export interface PasswordRequirement {
  id: string;
  label: string;
  test: (password: string) => boolean;
}

/** Single source of truth for both the live signup checklist and the
 * submit-time gate -- NIST 800-63B deliberately recommends against
 * composition rules (forced uppercase/number/symbol) in favor of just a
 * length floor plus (ideally) a breach-list check; a breach check needs a
 * real service/dataset this app doesn't have, so length is the one rule
 * here, not a placeholder for more that got cut. */
export const PASSWORD_REQUIREMENTS: PasswordRequirement[] = [{ id: "length", label: "At least 8 characters", test: (password) => password.length >= 8 }];
