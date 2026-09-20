# Security Policy

## Supported versions

Security fixes are made on the latest published minor release. Development
branches and older minors do not receive a guaranteed backport. Before the first
0.6 release is published, the repository has no supported release line.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability. Use GitHub's private
vulnerability-reporting form for this repository:

<https://github.com/AndrejOrsula/pyo3_bindgen/security/advisories/new>

If that form is unavailable, contact the maintainer using the address in the
workspace package metadata and include `pyo3_bindgen security` in the subject.
Do not send credentials, tokens, private keys, or unrelated personal data.

Include the affected version or commit, platform and Python/Rust versions,
impact, reproduction steps, and any proposed mitigation. Reports about generated
code should include the smallest safe Python surface or external IR that
reproduces the issue.

Please allow time for validation and a coordinated fix before public disclosure.
The project does not offer a bug bounty or a guaranteed response-time SLA.
