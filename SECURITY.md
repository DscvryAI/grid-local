# Security Policy

Grid Local reads AI coding session archives (Claude Code, Codex CLI,
and other supported tools) stored locally on the user's machine. It is
designed to be read-only with respect to that data and to never send
anything off the local machine. A vulnerability that breaks either of
those guarantees, or that exposes local session data to another
process or user, is a security issue.

## Reporting a vulnerability

Please do **not** open a public GitHub issue for security reports.
Instead, email **grid-local@dscvryai.com** with:

- A description of the issue and its potential impact.
- Steps to reproduce, including OS and app version.
- Any relevant logs (redact session content if it contains anything
  sensitive to you).

We aim to acknowledge reports within a few business days.

## Scope

In scope: anything that lets Grid Local read, write, or transmit data
it shouldn't (including data belonging to other AI coding tools it
integrates with), privilege escalation, or a crash triggerable by a
malformed session file from an untrusted source.

Out of scope: issues that require local admin/root access the attacker
already has, or that rely on the user's own machine already being
compromised.
