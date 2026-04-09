# Repo Sync Policy

Status: Active
Date: 2026-04-09
Owner: GitGraph Lab

## Why This Exists

Silent drift between code, contracts, and docs destroys trust.

## One-Line Policy

Every important surface needs one canonical source of truth, explicit derived surfaces, and machine-checked sync.

## Canonical To Derived Map

1. Architecture decisions
   Canonical: `docs/adr/*.md`
   Derived: `docs/adr/README.md` ADR index examples and references
2. Engineering standards
   Canonical: `docs/standards/*.md`
   Derived: `docs/standards/README.md`
3. Contributor operating contract
   Canonical: `AGENTS.md`
   Derived: cross-links in `docs/engineering-system.md` and standards docs

## Sync Rules

1. Required standards docs must exist.
2. `docs/standards/README.md` must list all required standards files.
3. `docs/adr/README.md` must list every existing ADR file name.
4. Missing required sections in standards docs fail guardrails.

## Enforcement

`scripts/enforce-guardrails.ps1` enforces this policy as part of `pnpm run guardrails`.
