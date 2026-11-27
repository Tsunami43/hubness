/** What an account may do: an admin everything (including accounts), a member what its permissions allow. */
export type Role = 'admin' | 'member';

/**
 * Parts of the hub a member can be given (chats in its projects are always available):
 * `terminal` — run commands, shells, preview and the environment; `settings` — hub-wide
 * settings (defaults, git access, voice input).
 */
export type Section = 'git' | 'files' | 'terminal' | 'kanban' | 'system' | 'harness' | 'settings';

/** A member's permissions; a `null` list means "all". */
export interface Permissions {
  /** Projects the member can open (and groups made only of them). */
  projects: string[] | null;
  sections: Section[];
  /** Agent kinds the member can start sessions with. */
  agents: string[] | null;
  models: string[] | null;
}

/** A hub account; `permissions` is set for members only. */
export interface User {
  id: string;
  username: string;
  createdAt: string;
  role: Role;
  permissions: Permissions | null;
}

/** `PUT /api/users/:id/access` (admins only). */
export interface UserAccess {
  role: Role;
  permissions?: Permissions | null;
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
