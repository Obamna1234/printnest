#!/usr/bin/env bash
# One-time GitHub setup for PrintNest (SPEC sections 8 and 9).
#
# Run from the repo root on your own computer, after `gh auth login`:
#     scripts/setup-repo.sh <your-github-username> [repo-name]
#
# What it does (all reversible in the GitHub UI):
#   1. Puts your username in CODEOWNERS, Cargo.toml, SECURITY.md, README.
#   2. Creates the public repo and pushes main.
#   3. Turns on private vulnerability reporting, secret scanning + push
#      protection, Dependabot alerts and security updates.
#   4. Sets Actions to read-only tokens; Actions may not approve PRs.
#   5. Protects main with rulesets: green CI + signed, linear history (no
#      bypass), PR + 1 code-owner approval; only admins may push v* tags.
#   6. Creates the `release` environment that needs your approval and only
#      accepts v* tags.
#   7. Creates the labels the workflows use.
#
# It never handles secrets. Add your Anthropic key yourself afterwards:
#     gh secret set ANTHROPIC_API_KEY
set -euo pipefail

owner="${1:-}"
repo="${2:-printnest}"

if [[ ! "$owner" =~ ^[A-Za-z0-9]([A-Za-z0-9-]{0,37}[A-Za-z0-9])?$ ]]; then
  echo "usage: $0 <github-username> [repo-name]" >&2
  exit 2
fi
if [[ ! "$repo" =~ ^[A-Za-z0-9._-]{1,100}$ ]]; then
  echo "invalid repo name: $repo" >&2
  exit 2
fi
command -v gh >/dev/null || { echo "Install the GitHub CLI (gh) first." >&2; exit 1; }
gh auth status >/dev/null || { echo "Run 'gh auth login' first." >&2; exit 1; }
[[ -f AGENTS.md && -f Cargo.toml ]] || { echo "Run this from the repo root." >&2; exit 1; }

full="$owner/$repo"
echo "==> Setting up $full"

# 1. Fill in placeholders.
files=(.github/CODEOWNERS .github/ISSUE_TEMPLATE/config.yml Cargo.toml SECURITY.md README.md)
for f in "${files[@]}"; do
  # Portable in-place edit (GNU and BSD sed).
  sed -e "s#@OWNER#@$owner#g" -e "s#OWNER/printnest#$full#g" "$f" > "$f.tmp" && mv "$f.tmp" "$f"
done

# 2. Create the repo and push.
if [[ ! -d .git ]]; then
  git init -b main
fi
git add -A
git -c user.useConfigOnly=true commit -m "PrintNest M0+M1: foundations, model, catalogue, CLI, window" || true
if ! gh repo view "$full" >/dev/null 2>&1; then
  gh repo create "$full" --public --source . --remote origin \
    --description "Free, offline, open-source designer for photo books, prints, frames, calendars and gifts" \
    --push
else
  git remote get-url origin >/dev/null 2>&1 || git remote add origin "https://github.com/$full.git"
  git push -u origin main
fi

api() { gh api -H "Accept: application/vnd.github+json" "$@" >/dev/null; }

# 3. Security features.
echo "==> Security features"
api -X PUT "repos/$full/private-vulnerability-reporting"
api -X PUT "repos/$full/vulnerability-alerts"
api -X PUT "repos/$full/automated-security-fixes"
api -X PATCH "repos/$full" --input - <<'JSON'
{
  "delete_branch_on_merge": true,
  "allow_merge_commit": false,
  "allow_rebase_merge": false,
  "allow_squash_merge": true,
  "security_and_analysis": {
    "secret_scanning": { "status": "enabled" },
    "secret_scanning_push_protection": { "status": "enabled" }
  }
}
JSON

# 4. Actions: read-only token by default; workflows may not approve PRs.
echo "==> Actions permissions"
api -X PUT "repos/$full/actions/permissions/workflow" \
  -f default_workflow_permissions=read -F can_approve_pull_request_reviews=false

# 5. Protect main and release tags. Three rulesets, so the solo-maintainer
#    exception (ruleset B) can only ever skip human REVIEW, never the
#    required checks, history rules or signatures in ruleset A.
echo "==> Ruleset A: main must pass every check (no bypass for anyone)"
api -X POST "repos/$full/rulesets" --input - <<'JSON'
{
  "name": "Main: checks, history and signatures",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [],
  "rules": [
    { "type": "deletion" },
    { "type": "non_fast_forward" },
    { "type": "required_linear_history" },
    { "type": "required_signatures" },
    { "type": "required_status_checks", "parameters": {
        "strict_required_status_checks_policy": true,
        "required_status_checks": [
          { "context": "Format" },
          { "context": "Test (ubuntu-latest)" },
          { "context": "Test (macos-latest)" },
          { "context": "Test (windows-latest)" },
          { "context": "Lockfile is up to date" },
          { "context": "cargo-deny" },
          { "context": "cargo-audit" },
          { "context": "cargo-vet" },
          { "context": "No network code in binaries" },
          { "context": "No unsafe in first-party code" },
          { "context": "Workflow security (CodeQL)" }
        ]
    } }
  ]
}
JSON

echo "==> Ruleset B: main needs a code-owner approval"
api -X POST "repos/$full/rulesets" --input - <<'JSON'
{
  "name": "Main: human review",
  "target": "branch",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
  "bypass_actors": [
    { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "pull_request" }
  ],
  "rules": [
    { "type": "pull_request", "parameters": {
        "required_approving_review_count": 1,
        "require_code_owner_review": true,
        "dismiss_stale_reviews_on_push": true,
        "require_last_push_approval": true,
        "required_review_thread_resolution": true
    } }
  ]
}
JSON
# The bypass in B lets an admin merge their OWN PR when they are the only
# maintainer (GitHub won't let you approve your own PR). It still needs a
# PR and every check in ruleset A. Remove it once a second maintainer joins:
#   Settings -> Rules -> "Main: human review" -> Bypass list.

echo "==> Ruleset C: only admins can create, move or delete v* tags"
api -X POST "repos/$full/rulesets" --input - <<'JSON'
{
  "name": "Release tags",
  "target": "tag",
  "enforcement": "active",
  "conditions": { "ref_name": { "include": ["refs/tags/v*"], "exclude": [] } },
  "bypass_actors": [
    { "actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always" }
  ],
  "rules": [
    { "type": "creation" },
    { "type": "update" },
    { "type": "deletion" },
    { "type": "non_fast_forward" }
  ]
}
JSON

# 6. Release environment: your approval + v* tags only.
echo "==> Release environment"
uid="$(gh api "users/$owner" --jq .id)"
api -X PUT "repos/$full/environments/release" --input - <<JSON
{
  "wait_timer": 0,
  "prevent_self_review": false,
  "reviewers": [ { "type": "User", "id": $uid } ],
  "deployment_branch_policy": { "protected_branches": false, "custom_branch_policies": true }
}
JSON
api -X POST "repos/$full/environments/release/deployment-branch-policies" \
  -f name='v*' -f type=tag || true

# 7. Labels.
echo "==> Labels"
make_label() { gh label create "$1" --color "$2" --description "$3" --repo "$full" --force >/dev/null; }
make_label "agent:build"  "5319e7" "Hand this issue to the coding agent"
make_label "needs-human"  "d93f0b" "The agent gave up; a person needs to look"
make_label "task"         "0e8a16" "Build task from the spec"
make_label "dependencies" "0366d6" "Dependency update"
make_label "security"     "b60205" "Security-relevant change"

cat <<EOF

Done. Two things left for you:

  1. Add your Anthropic API key so the agent and reviewer can run:
       gh secret set ANTHROPIC_API_KEY --repo $full

  2. Open https://github.com/$full/actions and check the first CI run is green.

Releases need a SIGNED, annotated tag (set up commit signing first:
https://docs.github.com/authentication/managing-commit-signature-verification):
    git tag -s v0.1.0 -m "PrintNest 0.1.0" && git push origin v0.1.0

Then open an issue from the "Build task" template and add the
'agent:build' label to hand it to the agent.
EOF
