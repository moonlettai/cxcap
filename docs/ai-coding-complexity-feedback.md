# AI coding agents have a complexity feedback problem

Frontier AI coding agents are remarkably good now. Give one a well-scoped task in a real repository and it will usually find the files, write the change, run the tests and hand you a green build. That's now routine.

That success changes the question worth asking. It's no longer only *can the AI coding agent complete this task?* It's also *what does the repository look like after the hundredth task, when every new AI coding agent session inherits the structure the previous sessions left behind?*

## Faster generation changes the economics

When writing code was slow, an engineer had to absorb every structural decision while writing it. The effort of typing acted as a brake: you felt a module getting too big because you kept scrolling through it.

AI coding agents remove most of that brake. Code is cheap to produce, so the scarce resource moves somewhere else, to *understanding*: whoever makes the next change, engineer or AI coding agent, has to rebuild a mental model of the code it's about to touch.

## Local correctness is not global structural health

An AI coding agent optimizes the task in front of it. Tests check that task's behavior. Neither is designed to answer a different question:

> Tests answer *"did the change work?"* The structural question is *"what complexity is this change interacting with, and is it adding more?"*

A change can pass every test and still add a dependency edge between two modules that used to be independent, duplicate a path instead of reusing one, or deepen a function that was already the hardest one in the file. None of that fails a build.

## Complexity compounds across iterations

One such change is harmless. The trouble is that AI coding agents make many of them, quickly, and each one looks reasonable in isolation. After enough iterations the codebase gets harder to work with, even though no individual step was wrong.

External data points in the same direction. GitClear, analyzing 623 million code changes from 2023 to 2026, reports duplicated code blocks up 81% and refactoring-style "moved" code falling to 3.8% of changed lines. It also finds new code connects to existing functions 35% less often than in 2023. GitClear is careful to say its headline "is not 'AI writes bad code'". These are correlations across a period of rising AI use, not proof of cause.

A peer-reviewed Carnegie Mellon study (MSR 2026) is more direct. In open-source projects that adopted Cursor, it found "a … transient increase in … velocity, along with a substantial and persistent increase in static analysis warnings and code complexity."

## Future AI coding agents must reason through that structure

The output of today's AI coding agent becomes the context of tomorrow's AI coding agent. Every extra coupling, cycle or duplicated path is something the next session has to discover, read and hold in context before it can change anything safely. And that session is often starting from zero, with no memory of why the structure looks the way it does.

## Billing makes that workload visible

This part is new. AI coding agent usage is metered, and vendors say plainly what drives the meter:

- Anthropic's Claude Code docs list "codebase size" among the factors that make costs "vary widely", and note that "token costs scale with context size."
- GitHub's Copilot billing docs: "A complex agentic session working across a large codebase will consume significantly more usage than a quick question in chat."
- OpenAI says Codex usage depends on the "size and complexity of your tasks", plus context, reasoning and tool use.

A controlled 2026 preprint by researchers at SonarSource, a code-quality vendor, isolates the effect. On clean and messy versions of the same code, Claude Code passed tasks equally often, but on the clean versions it used 7–8% fewer tokens and revisited files 34% less.

So technical debt used to be priced mainly in future developer time. Now it can also have a token bill: more context to ingest, more files to revisit, more retries. I want to be careful here. That's a mechanism, not a promise that any tool will cut your bill by some percentage.

## Why tests and review don't cover it

DORA's 2025 research found that AI adoption now goes with higher delivery throughput and still with lower stability. It also found that teams in loosely coupled architectures with fast feedback loops see the gains, while tightly coupled ones see "little or no benefit". DORA's framing is that AI amplifies what is already there.

Tests are behavioral verification. Code review is engineer judgment, and it's increasingly the bottleneck: DORA notes that time saved writing code is often spent again on "auditing and verification". What's usually missing is a cheap, independent, structural signal that someone (or something) can check *before* the change, not after the reviewer is already tired.

## Why I built CXCAP

I built CXCAP because I was using frontier AI coding agents heavily and kept seeing the same failure mode. Individual tasks succeeded. Tests passed. Each iteration looked reasonable. But after enough iterations the codebase became progressively harder to work with.

The AI coding agents were strong at the task directly in front of them, but they had no independent structural feedback loop telling them what complexity already surrounded the next change. That also started to matter economically: the harder the repository became to understand, the more context, reasoning and AI coding agent work future changes required.

I wanted something outside the model, local, deterministic and read-only, that could inspect the repository before another change was made. That became CXCAP.

## What it measures

CXCAP is a CLI. Point it at a repository and it parses Python, JavaScript/TypeScript and Rust with tree-sitter, then reports:

- **hotspots**: where size, branching and coupling concentrate;
- **dependents and transitive reach** of a file or folder (`--focus <path>`);
- **import cycles**, separating load-time cycles from lazy ones;
- **a bounded context set for a task described in plain English** (`--intent "<change>"`): likely touchpoints, the files around them, cross-boundary edges and cycles;
- **uncertainty**: places where dynamic imports, `getattr` dispatch or registries mean the static picture is incomplete.

It doesn't grade code, estimate effort or call a model. No index, no daemon, nothing uploaded. An AI coding agent can run it the same way an engineer does (the installer adds an agent skill for that).

A concrete run: on a public Django checkout I gave it the intent *"add session expiry to authenticated requests."* CXCAP's bounded context set (it considers at most 25 files) held 22 production files across 2 components, with 14 cross-boundary edges, 4 import cycles (1 load-time, 3 through function-level imports) and dynamic-analysis uncertainty in 10 places. It took under a second.

That doesn't mean Django is bad. It's a mature, well-engineered framework. It means a request that *sounds* local sits inside a wider reasoning surface, and an AI coding agent should see that before it edits, not after.

## Attempts to falsify it

A tool like this is only worth anything if it's wrong in visible, fixable ways. So I've been trying to break it:

- **It audits itself.** Run on its own source, CXCAP reports its own verdict as SEVERE, with 15 high warnings and a 22-block clone between two of its own parsers. A tool built to flatter wouldn't say that.
- **Historical intents.** Across 31 completed real changes in 7 repositories, CXCAP's top 10 candidates recovered about 70% of the files that actually changed (Recall@10 0.70, MRR 0.47). That's useful, not omniscient.
- **Bugs found while preparing this post.** Testing it against a dozen public repos turned up real problems. In Python, a qualified `if typing.TYPE_CHECKING:` guard was treated as a runtime import, which invents cycles. `from pkg import submodule` wasn't counted as a dependency, which undercounts dependents. And the intent context set is capped at 25 files without saying so. Fixes are in progress as public pull requests.

## Honest limitations

- **Static only.** Plugin systems, dependency injection, reflection and external consumers can be invisible; CXCAP flags what it can detect, not everything.
- **Supported languages only.** Python, JS/TS and Rust are scored. Other languages are listed as unscored, and if they dominate, the verdict is N/A rather than a misleading LOW.
- **No effort prediction.** It describes present structure, not how long a change will take.
- **`--intent` is lexical retrieval plus graph expansion**, a ranked starting point rather than ground truth. Domain vocabulary it doesn't recognize will lower its recall.
- A HIGH or SEVERE verdict is not a quality grade. Mature systems often score high because they're large and heavily depended on.

## Try to break it

The claim I'm making is narrow: *AI coding agents need a structural feedback loop, and one can be cheap, local and independent of the model.* If CXCAP is useless on your codebase, that's worth knowing too.

Run it on the repository you've built most heavily with AI, and give it your next real change:

```sh
curl -fsSL https://github.com/moonlettai/cxcap/releases/latest/download/install.sh -o install.sh
sh install.sh
cxcap audit . --intent "<your next real change>"
```

Then tell me where it's wrong: a false positive, a missed dependency, a confusing line, a repo where the analysis means nothing. Reports go in the [challenge issue](https://github.com/moonlettai/cxcap/issues/2). Criticize it, benchmark it, fork it, publish the failures. None of that needs my permission.

— Moonlett

*Disclosure: I built CXCAP. This article was drafted with AI assistance and edited by me; every figure links to its primary source.*

---

### Sources (all accessed 2026-09-24)
- GitClear, *The Maintainability Gap: AI Code Quality in 2026*: https://www.gitclear.com/the_ai_code_quality_maintainability_gap
- He, Miller, Agarwal, Kästner, Vasilescu, *Speed at the Cost of Quality…* (MSR 2026): https://arxiv.org/abs/2511.04427
- Trivedi & Schmitt (SonarSource), *Does Code Cleanliness Affect Coding Agents?* (preprint, May 2026): https://arxiv.org/abs/2605.20049
- Google DORA 2025: https://cloud.google.com/blog/products/ai-machine-learning/announcing-the-2025-dora-report
- DORA, *Balancing AI tensions* (Mar 2026): https://dora.dev/insights/balancing-ai-tensions/
- Anthropic, Claude Code costs: https://code.claude.com/docs/en/costs
- GitHub Copilot billing: https://docs.github.com/en/copilot/concepts/billing-and-usage/individuals/billing
- OpenAI Codex pricing: https://developers.openai.com/codex/pricing
- CXCAP Django run: django@a013c82, `cxcap audit . --intent "add session expiry to authenticated requests"` (cxcap 1.0.2)
