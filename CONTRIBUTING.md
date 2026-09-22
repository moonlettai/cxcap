# Contributing to CXCAP

CXCAP is intentionally falsifiable. The most valuable contribution is evidence that improves or limits the analysis.

## Fastest useful contribution

Run CXCAP on a substantial public repository and a real next change:

    cargo install cxcap
    cxcap audit . --intent "<your next real change>"
    cxcap audit . --focus <surprising target>

Report:

- the repository language and public URL;
- the command and CXCAP version;
- the relevant output;
- whether the result was useful, false-positive, incomplete, confusing, or not meaningful;
- what you expected it to show.

Do not submit private source, secrets, proprietary paths, or generated reports containing sensitive data. A sanitized reproduction is enough.

## Code changes

Before a cross-module change, use CXCAP to understand the likely structural surface. Keep changes focused, preserve the read-only contract, and add or update tests. Run the relevant test suite and formatter before opening a pull request.

Good contribution areas include:

- reproducible false positives or missed dependency edges;
- parser and language support gaps;
- intent-ranking failures with a public fixture;
- confusing output or documentation gaps;
- integration examples for coding-agent workflows.

Please open an issue before large changes. The public adversarial challenge is at https://github.com/moonlettai/cxcap/issues/2.