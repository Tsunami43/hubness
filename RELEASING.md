# Releasing

Releases are built by `.github/workflows/release.yml` from a `vX.Y.Z` tag. Each one
publishes, for Linux x86_64 and Apple silicon Macs:

- `hubness-X.Y.Z-<target>-full.tar.gz` — the hub with the web app inside;
- `hubness-X.Y.Z-<target>-client.tar.gz` — the client build (`tui`, `webapp`, `update`);
- a `.sha256` next to each archive, `SHA256SUMS` and `VERSION`.

## Steps

1. On a `release/X.Y.Z` branch from `develop` (Git Flow, see CONTRIBUTING.md), bump the
   version:
   - `version` under `[workspace.package]` in `Cargo.toml`, and the `=X.Y.Z` pins of the
     workspace crates under `[workspace.dependencies]`; run `cargo build` to update
     `Cargo.lock`;
   - `version` in `package.json`, `apps/server/package.json`, `apps/web/package.json` and
     `packages/shared/package.json`.
2. Add the release to `CHANGELOG.md`.
3. Commit (`chore: release X.Y.Z`), merge into `main` and back into `develop`.
4. Tag `main` and push the tag:

   ```sh
   git tag vX.Y.Z && git push --tags
   ```

The workflow fails early when the tag doesn't match the version in `Cargo.toml`. A tag with a
`-` (`v0.3.0-rc.1`) becomes a pre-release; GitHub's "latest" release, and so `install.sh` and
`hubness update`, skip pre-releases (install one with `--version 0.3.0-rc.1`).

## How installs pick it up

- `install.sh` and `hubness update` read `releases/latest/download/VERSION` (a plain file,
  no API rate limits) and download the archive of their own target and flavor, checking its
  sha256.
- `hubness update --check` exits with 10 when a newer release exists; `hubness update`
  replaces the running binary and restarts the hub service when it runs.
- `hubness --version` and `hubness server status` mention a newer release from a cached
  check (`~/.cache/hubness/update-check`, refreshed at most once a day by `server status`).

To try the update path without publishing anything, point `HUBNESS_RELEASE_URL` at a folder
with the same layout (`latest/download/VERSION`, `download/vX.Y.Z/<archives>`), as a
`file://` URL or over local HTTP; `install.sh` honors the same variable.
