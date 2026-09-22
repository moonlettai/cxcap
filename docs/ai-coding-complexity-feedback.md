# AI Coding Agents Have a Complexity Feedback Problem

AI coding agents are making implementation faster. That changes the bottleneck.

The difficult question is no longer only whether an agent can complete the task in front of it. It is whether the repository remains understandable after the hundredth task, when every new agent inherits the structure produced by the previous ones.

A change can pass its tests and still enlarge the reasoning surface for the next change. It can add another dependency edge, abstraction, duplicated path, or module that future agents must inspect before safely touching the code.

That is the gap between behavioral verification and structural feedback.

Tests answer: **did the change work?**

The complementary question is: **what complexity is this change interacting with?**

I built [CXCAP](https://github.com/moonlettai/cxcap) as a local, read-only attempt at that feedback loop. Given a repository and either a target path or a natural-language intent, it reports likely touchpoints, hotspots, direct dependents, transitive exposure, cycles, and static-analysis uncertainty. It does not estimate effort or grade a codebase. It exposes constraints before implementation.

## A reproducible proof

On a clean public Django checkout, the intent was:

> add session expiry to authenticated requests

CXCAP identified 22 production files and 3 verification files in the reasoning surface, with 14 cross-boundary edges, 4 cycles, and dynamic behavior that static analysis cannot fully resolve. The repository received a `SEVERE` structural verdict with 230 high warnings across 935 production files.

That does not mean Django is bad. It means a mature system can make a supposedly local change expensive to understand. That is exactly the kind of fact an agent should see before it edits.

The result is reproducible with:

```sh
cargo install cxcap
cxcap audit . --intent "add session expiry to authenticated requests"
```

## The economic angle

Structural complexity can require future agents to inspect more files, reconstruct more context, reason across more relationships, run more tools, and retry more often. No tool should promise a made-up savings percentage. But the mechanism is measurable:

> the output of today's agent becomes the context of tomorrow's agent.

Technical debt can now have a token bill.

## Try to falsify it

Run CXCAP on the repository you have built most heavily with AI. Give it your next real task. If it finds nothing useful, report that. If it finds something surprising, report that instead. False positives and missed exposure are more valuable than a flattering score.

The goal is not to replace tests, code review, or engineering judgment. It is to add an independent structural feedback loop before complexity quietly compounds.

[Open the adversarial challenge](https://github.com/moonlettai/cxcap/issues/2).
