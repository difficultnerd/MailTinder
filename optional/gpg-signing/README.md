# Optional layer: GPG commit signing

Use where verified commit authorship is required. Not part of the core template.

Install: `optional/install.sh gpg-signing`, then `tools/require_signed_commits.sh` (needs admin rights).
Add `signed-commits` to the required checks in `tools/apply_branch_protection.sh`.

## Contributor setup

```sh
gpg --full-generate-key                      # Ed25519, your GitHub email
gpg --list-secret-keys --keyid-format=long   # note the key ID
gpg --armor --export <KEY_ID>                # add at GitHub > Settings > SSH and GPG keys
git config --global user.signingkey <KEY_ID>
git config --global commit.gpgsign true
```

Git also accepts SSH signing keys (`gpg.format ssh`), which GitHub verifies the same way and which
need less setup.

Squash and merge via the GitHub web UI produces a commit GitHub signs itself, so it passes.
