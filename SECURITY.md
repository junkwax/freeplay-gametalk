# Security Policy

## Supported Versions

Security fixes are prioritized for the latest tagged release and the `main`
branch. Older releases are not normally backported unless a fix is low risk and
needed for active users.

## Reporting a Vulnerability

Please report suspected vulnerabilities privately through GitHub:

https://github.com/junkwax/freeplay-gametalk/security/advisories/new

If private reporting is unavailable, open a GitHub issue with a short, general
description and ask for a private contact path. Do not include exploit details,
tokens, private service URLs, Discord webhook URLs, crash logs containing auth
headers, or other secrets in a public issue.

Useful reports include:

- The affected version or commit.
- The operating system and build type.
- Steps to reproduce, using sanitized test data.
- Impact and any suspected attacker requirements.

We will acknowledge actionable reports as soon as practical, investigate with
the reporter when needed, and publish fixes or mitigation notes once users can
update safely.

## Scope

This repository contains the Freeplay client and release packaging. Server-side
issues in sibling services should be reported against the relevant service
repository when known, or through this repository's private advisory form if you
are unsure.

## Handling Secrets

Do not attach ROMs, private `.env` files, Discord OAuth tokens, webhook URLs,
relay credentials, or full unsanitized client logs to public reports. When logs
are needed, remove authorization headers, deep-link room/session identifiers,
and any local paths or account identifiers you do not want shared.
