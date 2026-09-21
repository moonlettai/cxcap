# CXCAP

**See complexity before it compounds.**

A seemingly small change can interact with coupling, import cycles, public
contracts, dependencies, and a large context surface — and the cost only
shows up after the code is written. CXCAP exposes that evidence *before*
implementation, so the plan can stay narrow.

## Benefit

Point CXCAP at a repository and it reports, from the code as it exists now:

- likely touchpoints for a planned change (`--intent`, `--focus`);
- what else the change could affect (direct and transitive exposure);
- hotspots, import cycles, structural duplication, and planning constraints;
- uncertainty it cannot see (dynamic behavior, external consumers).

It does not decide what to build and does not estimate effort. It gives
humans and coding agents measured facts to plan against.

## When to use it

- Investigating an unfamiliar subsystem.
- Planning or scoping a feature, refactor, or API change.
- A "small" change that keeps spreading.
- AI-agent planning, change-plan review, or pre-completion review.
- A post-change check that complexity did not grow disproportionately.

## What it returns

`cxcap audit` prints a verdict (LOW / MODERATE / HIGH / SEVERE, or N/A when
the repository is dominated by unscored languages), ranked hotspots,
folder concentration, warnings that always name the file and the observed
fact, import cycles (small eager cycles HIGH, large or lazy tangles WATCH),
clone pairs to fix in all places, and constraints for the next decision.

Verdicts are change/complexity constraint signals, not software-quality grades:
a mature, well-engineered repository may legitimately read HIGH or SEVERE.

`--focus <path>` assesses a proposed change area: its complexity, direct
external dependents, transitive reach, hotspots inside it, and cycles
passing through it. Areas that are purely tests, examples, generated code,
or type declarations say so instead of alarming.

`--intent "<change description>"` starts from words instead of a path: it
ranks likely implementation files from path and symbol vocabulary, expands
one hop along the dependency graph, and reports the reasoning surface
(production and verification files, components, cross-boundary edges,
cycles) plus static-analysis uncertainty — evidence, never an estimate.

`--json` emits the same report machine-readable for agents.

## Evidence

Measured with CXCAP v1.0.0, full `audit --json`, production files only.
Pinned revisions, Apple M1 Pro (8 cores), single runs; `elapsed` includes
parallel analysis on all cores.

| project | rev | verdict | prod files | prod cx | avg | cycles | warnings (HIGH) | elapsed |
|---|---|---|---|---|---|---|---|---|
| requests | `dae7ef6` | SEVERE | 21 | 919 | 43.8 | 1 eager | 28 (9) | 0.3s |
| django | `446d9cf` | SEVERE | 935 | 28,696 | 30.7 | 24 | 828 (230) | 6.9s |
| pytest | `6a0de9b` | SEVERE | 94 | 6,520 | 69.4 | 2 | 171 (70) | 1.0s |
| pip | `2b28a81` | SEVERE | 166 | 5,229 | 31.5 | 3 | 209 (60) | 0.8s |
| slate | `279f35f` | SEVERE | 194 | 3,574 | 18.4 | 4 | 115 (43) | 0.6s |
| vue | `4ab865a` | SEVERE | 293 | 11,872 | 40.5 | 7 | 357 (133) | 1.1s |
| typescript | `f29aeb9` | SEVERE | 77 | 7,064 | 91.7 | 2 | 106 (36) | 19.7s |
| cal.com | `54343aa` | SEVERE | 4,331 | 53,885 | 12.4 | 8 | 1,491 (562) | 11.2s |
| nushell | `9fc5f8d` | SEVERE | 1,528 | 52,979 | 34.7 | 12 | 1,106 (354) | 5.2s |
| DefinitelyTyped | `ca965dd` | HIGH | 58 | 1,606 | 27.7 | 6 | 29 (7) | 19.3s |

Spot-checked findings: requests' 7-module eager cycle is real mutual
initialization; django's ORM core cycle survived a resolver fix that
removed phantom edges; TypeScript's only real eager cycle is 3 modules
(a 76-member cycle lives entirely in test-fixture copies and reads WATCH).
DefinitelyTyped's small eager cycles sit in test-example directories and
correctly read WATCH instead of HIGH.

Honest limitations of these numbers: verdicts summarize the scored
languages only (Python, JavaScript/TypeScript, Rust); see below. Changed
files from real historical changes were used separately to validate
`--intent` retrieval (31 changes, 7 repositories: Recall@10 0.70, MRR 0.47).

## Installation

Recommended (prebuilt binary + agent skill + editor link, non-interactive):

```sh
VER=1.0.0
OS=$(uname -s | tr '[:upper:]' '[:lower:]' | sed 's/darwin/apple-darwin/;s/linux/unknown-linux-gnu/')
ARCH=$(uname -m | sed 's/arm64/aarch64/')
curl -fsSL "https://github.com/moonlettai/cxcap/releases/download/v${VER}/cxcap-${VER}-${ARCH}-${OS}.tar.gz" -o cxcap.tar.gz
tar xzf cxcap.tar.gz
sh install.sh --binary=./cxcap
cxcap --version
```

This installs `cxcap` to `~/.local/bin` (override with `--prefix=DIR`),
the `cxcap-development` skill to `~/.agents/skills/cxcap-development`, and
links it into `~/.claude/skills`. Re-running is safe and idempotent.
Binary only:

```sh
sh install.sh --binary=./cxcap --no-skill
```

Or build from source (requires Rust stable):

```sh
cargo install --path .
```

Prebuilt binaries target macOS (Apple Silicon, Intel) and Linux
(x86-64, ARM64) with published checksums. The Intel macOS archive is
built from the same source but was not executed before release (no
compatible execution environment was available); please verify
`cxcap --version` and an audit on your machine and open an issue if
anything fails. Windows is not a supported 1.0 target.

## Usage

```sh
cxcap audit .                          # whole repository
cxcap audit ../project                 # other locations (absolute or relative)
cxcap audit . --focus src/auth         # a proposed change area
cxcap audit . --intent "add session expiry"   # from a change description
cxcap audit . --json                   # machine-readable report for agents
cxcap audit . --jobs 1                 # serial mode (output is identical)
cxcap update                           # checksum-verified upgrade
cxcap update --check                   # report availability without installing
```

## Agent skill and updating

The installer places a `cxcap-development` skill for coding agents
(`~/.agents/skills/cxcap-development`, symlinked into
`~/.claude/skills`). It triggers on unfamiliar code, nontrivial plans,
features, architecture, refactors, dependencies, public APIs,
cross-module changes, plan review, and pre-completion checks.

CXCAP analysis itself works fully offline. At most once per day, an audit
may perform a short non-blocking release-version check (silent when
offline or failing); disable it with `CXCAP_NO_UPDATE_CHECK=1`.
`cxcap update` downloads a checksum-verified, smoke-tested release and
replaces the binary atomically where possible; a failed update keeps the
working binary. Updates refresh the skill only for installations that
opted into it.

## Supported languages and platforms

- Python (including analyzable notebooks), JavaScript / TypeScript
  (including embedded scripts in `.astro`, `.vue`, `.svelte`), Rust —
  all AST-based.
- Other languages are listed as unscored, never silently ignored. A
  project dominated by unscored code receives N/A instead of a
  misleading LOW.
- macOS and Linux (Intel macOS: build-only verification, see above).
  Windows is not a supported 1.0 target.

## Limitations and privacy

- Static analysis only: runtime imports, reflection, registries, and
  plugin loading are invisible by construction and reported as
  uncertainty.
- External consumers of public contracts cannot be counted from a local
  checkout; internal fan-in may understate compatibility exposure.
- Parse failures fall back to line counts and are flagged, never fatal.
- Nothing here predicts engineering effort.
- Privacy: analyzed repositories are read-only (files are only opened
  for reading; nothing is written to the target). No persistent
  repository index or database is created. Source code is never
  uploaded; the only network use is the daily release check and
  explicit updates (see above).

## License

MIT. See `LICENSE`.
