/** A hub account (all accounts have the same rights). */
export interface User {
  id: string;
  username: string;
  createdAt: string;
}

/**
 * `GET /api/auth/status`: `required` — the hub has accounts and asks for a sign-in; `setup` —
 * no accounts yet (the first one can be created from the hub's own machine); `user` — who is
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
