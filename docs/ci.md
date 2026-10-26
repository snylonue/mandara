# Continuous integration

The GitHub Actions workflow builds the server, frontend, and all in-repository
plugins on x86_64 Linux, then runs `nix flake check`, including the overlay
check and the NixOS VM integration test. It runs on pushes, pull requests,
and manual dispatches. The checked-in flake lock is never updated by CI.

## oranc binary cache

CI runs its own [oranc](https://github.com/linyinfeng/oranc) service container
and stores Nix build outputs in `ghcr.io/snylonue/mandara-nix-cache` using
[oranc-action](https://github.com/linyinfeng/oranc-action). Forks automatically
use their own lowercased owner and repository name with a `-nix-cache` suffix.

Only the default branch can upload cache entries. Pull requests and other
branches can read the public cache using `NIX_PUBLIC_KEY`, but never receive
the signing key. Configure `NIX_PUBLIC_KEY` for read-only jobs. Without a
signing key, uploads are skipped.

Upstream warns that the registry schema is unstable and that this use of GHCR
may violate GitHub's terms of service. Review the upstream README before
enabling uploads.

### One-time repository setup

1. Generate a signing key outside the checkout. Keep it private and back it up;
   never commit it or paste it into logs. Run these commands in Bash:

   ```sh
   umask 077
   key_dir="$(mktemp -d)"
   nix key generate-secret --key-name mandara-nix-cache-1 > "$key_dir/secret"
   nix key convert-secret-to-public < "$key_dir/secret" > "$key_dir/public"
   ```

2. In the repository's **Settings → Secrets and variables → Actions**, add:

   | Kind | Name | Value |
   |------|------|-------|
   | Repository secret | `NIX_SIGNING_KEY` | Contents of the generated secret key |
   | Repository variable | `NIX_PUBLIC_KEY` | Contents of the generated public key |

3. Initialize the cache once from a trusted machine, using a GitHub personal
   access token (classic) with `write:packages`. Substitute the repository owner
   and cache package name when configuring a fork:

   ```sh
   export ORANC_USERNAME=snylonue
   read -rsp 'GitHub token: ' ORANC_PASSWORD; echo
   export ORANC_PASSWORD
   export ORANC_SIGNING_KEY="$(< "$key_dir/secret")"
   nix run github:linyinfeng/oranc -- push --allow-immutable-db \
     --registry ghcr.io --repository snylonue/mandara-nix-cache initialize
   unset ORANC_PASSWORD ORANC_SIGNING_KEY
   ```

4. In the owner's GitHub **Packages** page, open `mandara-nix-cache`, change its
   visibility to **Public**, and grant this repository **Write** access under
   **Manage Actions access**. Anonymous cache reads require a public package.

5. Push to the default branch or manually dispatch CI on that branch. Uploads
   use the workflow's `GITHUB_TOKEN` with `packages: write`; no registry PAT is
   stored in Actions. oranc-action uploads newly built store paths in its
   post-job step. The cache is intentionally not initialized automatically,
   so the public visibility and repository access are set before CI uses it.
