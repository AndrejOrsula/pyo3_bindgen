# Documentation

- [Release plan and checklist](release_checklist.md): sole unfinished-work queue, release gates, evidence, and publication procedure.
- [Architecture decisions](ARCHITECTURE_DECISIONS.md): ownership and compatibility boundaries.
- [External-IR contract](EXTERNAL_IR_LOWERING_CONTRACT.md): wire validation and generated mappings.
- [Configuration matrix](CONFIG_CLI_POLICY_MATRIX.md): Config, CLI, macro, and consumer dependencies.
- [Resource budgets](RESOURCE_BUDGETS.md): limits and measurement semantics.
- [Observability contract](OBSERVABILITY_CONTRACT.md): events, correlation, and output policy.
- [Producer support](PRODUCER_SUPPORT.md): naming protocol and report/provenance utilities.
- [0.6 release notes](release_notes_0.6.md): consumer migration and release announcement source.
- [Schema migration pointer](EXTERNAL_IR_SCHEMA_ADDITIONS.md): retained downstream link; definitions live in the IR contract.

Repository entry points: [README](../README.md), [agent guide](../AGENTS.md),
[package map](../crates/README.md), [security policy](../SECURITY.md).
Code/tests govern behavior. Update contracts with implementation; keep completed plans and
reports in Git history. Run `scripts/check-docs.sh` after documentation changes.
