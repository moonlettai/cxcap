# CXCAP self-audit receipt

This is a reproducible public proof story, not a quality score.

Run against a clean checkout of CXCAP with `cxcap 1.0.2`:

    cxcap audit . --json
    cxcap audit . --intent "improve the intent analysis output before the next agent change" --json

Observed on the public checkout used for the launch proof:

- verdict: `SEVERE`
- 40 files total; 33 code files; 27 production files; 6 test files
- 11,283 code lines
- 1,603 production complexity; 1,951 total complexity
- 689 functions
- 43 warnings on the intent run; 15 HIGH warnings in the verdict explanation
- 0 reported cycles in the intent run

`SEVERE` is a structural constraint/exposure signal, not a claim that the project is bad. CXCAP does not exempt its own implementation from its own analysis. The output also reports static-analysis limitations and read-only behavior.

Numbers can change after a release. Re-run the commands above on the current public checkout rather than treating this receipt as a permanent benchmark.