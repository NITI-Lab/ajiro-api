# Security Policy

## Reporting a vulnerability

Please do not open a public issue for security problems. Report them privately through GitHub: open the repository's **Security** tab and choose **Report a vulnerability** (private vulnerability reporting) at https://github.com/niti-lab/ajiro-api/security/advisories/new.

Include the version (`ajiro-api --version`), your OS, and steps to reproduce. We aim to acknowledge reports within a few business days.

## Supported versions

Only the latest released version receives security fixes.

## Scope notes

- `ajiro-api` runs locally and sends your requests only to the `base_url` you configure. It does not upload API definitions or test results.
- Secrets belong in `.env` (referenced as `{{ env.NAME }}`) and are masked as `***` in output. A leak of a secret in logs, reports or generated files is a security bug; please report it.
- Anonymous usage statistics are off by default and contain only command counts.
