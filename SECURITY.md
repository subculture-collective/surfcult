# Security Policy

## Reporting a vulnerability

Do not report vulnerabilities in public issues. Contact the maintainers of the `subculture-collective` GitHub organization privately.

Include:

- the affected repository and commit
- what an attacker could do
- steps to reproduce
- the affected configuration or deployment
- a suggested fix, if you have one

Projects created from this template should add their own contact details and supported versions before production use.

## Secret handling

Never commit:

- `.env` or `.env.*` files, except `.env.example`
- API tokens or OAuth secrets
- SSH keys or deploy keys
- database dumps containing real data
- private certificates or signing keys
- webhook secrets

Use placeholders in docs and examples. When you report a problem, name a secret by its key, never by its value.

## Dependencies

Projects created from this template should:

- commit lockfiles when the stack expects them
- pin container base images deliberately
- document how to update dependencies
- run dependency and container scans where appropriate
- keep generated caches out of git
