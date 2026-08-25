// Shared between server code (route handlers, session helpers) and client
// code (the auth forms) -- plain types have no runtime footprint, so one
// definition is safe to import from both sides rather than duplicating it.

export interface SessionUser {
  id: number;
  email: string;
  first_name: string;
  last_name: string;
  tenant: string;
  avatar_url: string | null;
  handle: string | null;
  bio: string;
  location: string;
  theme_pref: string;
  default_search_scope: string;
  show_gotcha_callouts: boolean;
  graph_layout_algorithm: string;
  two_factor_enabled: boolean;
  onboarding_completed: boolean;
}

export interface AuthResponse {
  user: SessionUser;
  session_token: string;
}

/** What `POST /auth/login` returns instead of `AuthResponse` when the account has 2FA enabled -- no session capability yet, just a handle to complete via `POST /auth/login/2fa`. */
export interface TwoFactorChallenge {
  two_factor_required: true;
  challenge_token: string;
}
