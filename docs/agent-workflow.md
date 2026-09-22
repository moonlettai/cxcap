# CXCAP in an AI coding workflow

CXCAP is most useful before an agent edits a repository and again before the work is declared complete.

## Before the change

Give the agent the real task, then map the likely structural surface:

    cxcap audit . --intent "add session expiry to authenticated requests"

Read the named touchpoints, direct dependents, transitive exposure, cycles, hotspots, and uncertainty. Use that evidence to decide whether the task is genuinely local or needs a broader plan.

If the likely target is known:

    cxcap audit . --focus src/auth

Do not treat the verdict as a quality grade. A HIGH or SEVERE result means the change intersects stronger structural constraints; it does not mean the repository is bad.

## After the change

Run the focused audit again and compare the evidence:

    cxcap audit . --focus src/auth --json > /tmp/cxcap-after.json

Then run the repository's normal tests, build, and lints. CXCAP complements those checks:

- tests ask whether behavior works;
- CXCAP asks what structure the change interacts with;
- code review applies domain and product judgment.

## A useful agent instruction

Add this to the repository's agent guidance when appropriate:

> Before changing a cross-module or dependency-sensitive area, run `cxcap audit . --intent "<task>"` or `cxcap audit . --focus <target>`. Use the result to identify touchpoints, dependents, cycles, hotspots, and static-analysis uncertainty. Do not optimize for a lower CXCAP score; minimize only unnecessary complexity while preserving required structure. Re-run the relevant audit before completion.

## Privacy and limits

CXCAP is local and read-only toward the analyzed repository. It does not upload source or require an account, model, index, or daemon. Static analysis can miss dynamic loading, reflection, registries, plugin systems, external consumers, and unsupported languages. Use normal search, runtime knowledge, and project verification alongside it.

For an adversarial trial, run CXCAP on a substantial repository you have built heavily with an AI agent and give it your next real task. Report useful findings, false positives, missed exposure, or cases where it adds no value: https://github.com/moonlettai/cxcap/issues/2