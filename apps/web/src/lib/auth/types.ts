// Shared between server code (route handlers, session helpers) and client
// code (the auth forms) -- plain types have no runtime footprint, so one
// definition is safe to import from both sides rather than duplicating it.

export interface SessionUser {
  id: number;
  email: string;
  first_name: string;
  last_name: string;
  tenant: string;
}

export interface AuthResponse {
  user: SessionUser;
  session_token: string;
}
