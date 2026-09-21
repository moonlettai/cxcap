---
name: cxcap-development
description: >
  Use CXCAP proactively before planning, scoping, refactoring, reviewing, or
  completing nontrivial code changes—especially in unfamiliar code, API or
  dependency work, architecture changes, cross-module edits, or when a small
  change is spreading. CXCAP identifies likely touchpoints, hotspots, runtime
  coupling, cycles, transitive exposure, context surface, and static-analysis
  uncertainty. Use it to constrain scope and verification, never to optimize a
  score.
---

# CXCAP development

Use CXCAP as a **pre-change map** and **post-change regression check**. It is
read-only toward the analyzed repository and uploads no source code.

## 1. Choose the smallest useful audit

- **Need a repository map** → `cxcap audit .`
- **Know the task, not the files** → `cxcap audit . --intent "<task>"`
- **Know the file or area** → `cxcap audit . --focus <path>`
- **Change already implemented** → rerun `--focus` on the touched area; use a
  whole-repo audit when architecture, dependencies, or module boundaries moved.

Use plain output first. Add `--json` when you need machine parsing, a large
report, or a before/after comparison.

If CXCAP prints an update notice, update it unless the environment forbids
network access.

## 2. Read evidence before acting

Prioritize named files and relationships over the overall verdict.

- **Hotspot / dense function** → keep the diff narrow and verify that behavior.
- **High fan-in / transitive reach** → preserve the interface and check callers.
- **Cycle through the target** → treat the cycle as one coordination surface.
- **Cross-component exposure** → expect boundary coordination; avoid widening
  the change casually.
- **Verification/generated surface** → do not let fixture volume drive the
  production plan.
- **Static uncertainty** → supplement CXCAP with project search, tests, runtime
  knowledge, or framework-specific checks.

`--intent` is ranked discovery, not ground truth. If its candidates are weak,
continue normal investigation; once you find the target, use `--focus`.

`--focus` reports static exposure, not every runtime consumer. Reflection,
registries, dependency injection, plugins, dynamic imports, and external users
can be invisible.

## 3. Plan for the minimum necessary complexity

- Make the smallest change that satisfies the requirement.
- Reuse existing boundaries before introducing new abstractions.
- Add compatibility layers, dependencies, files, or indirection only when the
  requirement or observed structure justifies them.
- Preserve necessary complexity; remove only complexity that is genuinely
  unnecessary.
- Never weaken behavior, tests, analysis, or maintainability to improve a
  CXCAP score.

A HIGH or SEVERE verdict is not a quality failure. It means the measured area
needs stronger scope control and verification.

## 4. Verify before completion

1. Run the project's normal tests, build, lint, or type checks.
2. Rerun the smallest relevant CXCAP audit.
3. Compare with the pre-change evidence when available.
4. Investigate any unexplained new hotspot, cycle, coupling increase, or
   exposure growth.
5. Finish when remaining complexity is justified by functionality and the
   project's own validation is green.

## Gotchas

- CXCAP does not estimate implementation time or effort.
- Type-only references do not count as runtime ripple exposure.
- Unsupported implementation languages are disclosed; dominant unscored code
  can produce `N/A` rather than a misleading LOW verdict.
- Audits work offline. A short daily release check may use the network; disable
  it with `CXCAP_NO_UPDATE_CHECK=1` when required.
- If `cxcap` is unavailable on PATH, do not block the coding task: report that
  CXCAP could not be run and continue with the project's normal investigation
  and validation tools.
