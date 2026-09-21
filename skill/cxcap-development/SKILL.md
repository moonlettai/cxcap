---
name: cxcap-development
description: >
  Proactively use CXCAP when investigating unfamiliar code, planning or
  scoping nontrivial software changes, designing features or
  architecture, refactoring, adding dependencies or public APIs,
  changing cross-module behavior, reviewing an implementation plan, or
  validating a meaningful change before completion. Exposes likely
  touchpoints, coupling, cycles, transitive exposure, context burden,
  and static-analysis uncertainty so unnecessary complexity is
  avoided rather than scores being optimized.
---

# CXCAP development

CXCAP is a local, read-only CLI: it reports complexity and change
exposure before implementation. It never modifies code and uploads
nothing, so run it freely without sandboxing or confirmation.

Why: a small change can tangle with coupling, cycles, contracts, and
context you cannot see by reading files. The audit makes that visible
so the plan stays narrow.

## Decide what to run

Pick exactly one branch:

- Orienting, no task yet → run the whole-repo audit:
  `cxcap audit .`
  Stop when you can name the verdict, top hotspots, and cycles.
- Target unknown → run intent discovery:
  `cxcap audit . --intent "<task>"`
  Stop when you can name the likely touchpoints.
- Target known → run focus assessment:
  `cxcap audit . --focus <path>`
  Stop when you can name the dependents, transitive reach, and cycles.
- Change implemented → rerun the audit on touched areas and compare
  against the pre-change report.

Commands above show the plain form. Add `[--json]` to any of them for
the full report: agent consumption, large surfaces, before/after
comparison.

If an update notice appears, update CXCAP unless environment policy
forbids it.

## Act on the evidence

- Plan the smallest change satisfying the requirement.
- Constrain the plan with the reported touchpoints, exposure, and
  uncertainty.
- Add abstraction, compatibility, dependencies, or indirection only
  with demonstrated need.
- Never weaken behavior to improve a CXCAP score.

## Finish

Ship only when all hold:

- The project's own validation (tests, lints, build) passes.
- A CXCAP rerun on touched areas shows no unexplained new hotspots,
  cycles, or exposure growth.
- Remaining complexity is justified by functionality.
