# Contributing

## Setup

```sh
corepack enable
pnpm install
pnpm dev
```

## Branches (Git Flow)

- `main` — released versions only, every merge is tagged (`v0.1.0`).
- `develop` — integration branch, the base for new work.
- `feature/<short-name>` — branched from `develop`, merged back with `--no-ff`.
- `release/<version>` — branched from `develop` to prepare a release (version bump,
  changelog), merged into `main` and back into `develop`.
- `hotfix/<version>` — branched from `main` for urgent fixes, merged into `main` and `develop`.

## Commits

Use [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(server): add kanban card API
fix(web): keep editor scroll position on save
docs: describe preview ports
```

Common types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `build`, `chore`.
Scopes: `server`, `web`, `shared`, `agents`, `git`, `db`.

## Before opening a pull request

- `pnpm check` passes.
- `pnpm test` passes; new server behaviour has tests.
- New UI strings are added to both `apps/web/src/i18n/en.ts` and `ru.ts`.
- Database changes are a new migration in `apps/server/src/db/migrations.ts`.
- `CHANGELOG.md` has an entry under "Unreleased".
