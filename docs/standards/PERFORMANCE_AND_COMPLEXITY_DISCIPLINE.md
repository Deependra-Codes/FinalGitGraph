# Performance And Complexity Discipline

Status: Active
Date: 2026-04-09
Owner: GitGraph Lab

## Why This Exists

Correctness on tiny fixtures is not enough for repository-scale workloads.

## One-Line Policy

Time, space, and data-structure quality are release criteria for scale-sensitive paths.

## Core Rules

1. Start with workload shape (input size, growth path, read/write/scan frequency).
2. Classify path as hot, warm, or cold before choosing structures.
3. Record dominant operations and expected cost profile for hot paths.
4. Reject avoidable bad asymptotics in repo-scale loops.
5. Treat memory growth and allocation behavior as first-class.
6. Prefer measured claims over guessed claims for performance work.
7. Allow simpler slower implementations only when path is cold and tradeoff is explicit.

## Expected Artifacts For Scale-Sensitive Changes

1. Intent with workload assumptions.
2. Spec with chosen structure and key tradeoffs.
3. Verification record with measured vs inferred claims.

## Review Questions

1. What dominates cost?
2. Is chosen structure explicit and justified?
3. Is any avoidable full-rescan behavior present?
4. Who owns maintenance of added index/cache state?
5. What was measured and what remains inferred?
