/** The hub's one account (hubness is single-user: it is set at the first sign-in). */
export interface User {
  id: string;
  username: string;
  createdAt: string;
}

/** `PUT /api/auth/password`: a new password for the account; ends its other sessions. */
export interface PasswordChange {
  currentPassword: string;
  password: string;
}

/**
 * `GET /api/auth/status`: `required` — the hub has its account and asks for a sign-in; `setup` —
 * no account yet (it can be created from the hub's own machine); `user` — who is
 * signed in with this request.
 */
export interface AuthStatus {
  required: boolean;
  setup: boolean;
  user: User | null;
}

/** `POST /api/auth/login` and `/api/auth/setup`: the session token for API clients (browsers get a cookie). */
export interface LoginResponse {
  token: string;
  user: User;
}
