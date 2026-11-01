# Repository rulesets

The protection rules for `main` and release tags, as GitHub ruleset
exports. `scripts/setup-repo.sh` applies them with the GitHub CLI; to apply
one by hand: **Settings → Rules → Rulesets → New ruleset → Import a
ruleset**, then pick the file.

| File | What it enforces |
| --- | --- |
| `main-checks.json` | Every CI and security check must pass; signed commits; linear history; no force-push or deletion. No bypass for anyone. |
| `main-review.json` | Changes reach `main` only through a pull request with a code-owner approval. Admins may merge their own PRs (solo maintainer); remove that bypass once a second maintainer joins. |
| `release-tags.json` | Only admins can create, move or delete `v*` tags. |

Changes here need CODEOWNERS review (covered by `/.github/`).
