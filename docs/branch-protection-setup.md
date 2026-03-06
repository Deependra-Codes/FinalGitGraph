# Branch Protection Setup (GitHub)

Use this once in repository settings to make guardrails non-bypassable.

## Target Branch

1. Protect `main` (or your default branch).

## Required Settings

1. Require a pull request before merging.
2. Require approvals: minimum 1.
3. Dismiss stale approvals on new commits.
4. Require conversation resolution before merge.
5. Require status checks to pass before merging.

## Required Status Checks

1. `Backend Quality (ubuntu-latest)`
2. `Backend Quality (windows-latest)`
3. `cargo-audit`

## Recommended Additional Controls

1. Require branches to be up to date before merging.
2. Restrict who can push to matching branches.
3. Include administrators in protection rules.
4. Disable force pushes.
5. Disable branch deletion.
