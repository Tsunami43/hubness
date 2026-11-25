# Contributing

## Setup

```sh
pnpm install --frozen-lockfile
pnpm dev
```

The pnpm version is pinned in `package.json` (`packageManager`). Without a global pnpm,
run it through npx: `npx -y pnpm@11.29.0 install --frozen-lockfile`, `npx -y pnpm@11.29.0 dev`.
Corepack is not required.

## Dependencies

- JavaScript dependencies are pinned to exact versions (`saveExact` in
  `pnpm-workspace.yaml`, `save-exact` in `.npmrc`); versions published less than a day ago
  are not resolved (`minimumReleaseAge`). Check with `pnpm outdated -r` and `pnpm audit`.
- Rust dependencies are declared once in `[workspace.dependencies]` of the root
  `Cargo.toml` with exact versions (`=x.y.z`) and referenced with `.workspace = true`;
  `Cargo.lock` is committed, builds and installs use `--locked`. The MSRV is the
  workspace `rust-version`. Check with `cargo audit` and `cargo tree -d`.

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
