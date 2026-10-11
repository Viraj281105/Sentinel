# Security policy

Sentinel changes files on people's computers, so security reports are taken seriously.

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

Use GitHub's private vulnerability reporting for this repository: open the repository's
**Security** tab and choose **Report a vulnerability**. Include what you found, how to reproduce
it, the Sentinel version (Settings shows it) and your Windows version.

*(Maintainer: this works only when "Private vulnerability reporting" is enabled under the
repository's Settings, Code security. If you would rather use an e-mail address, put it here.)*

This is a single-maintainer project, so there is no guaranteed response time. Reports will be
acknowledged as soon as possible, fixed in the order of their impact, and credited if you wish.

## What counts as a vulnerability

Anything that lets Sentinel, or content it reads, do what its safety model forbids. Examples:

- Getting a protected location, or anything inside one, moved, listed for cleanup or restored
  over, for instance with a crafted path, a junction or symlink, a short (8.3) name, an
  alternate data stream or a device name.
- Getting Sentinel to remove or overwrite something it was not asked to, or without the
  confirmation step.
- A change that bypasses the audit log, or hides tampering with it.
- Sending data off the computer (for example through the AI assistant), or reaching a
  non-local address.
- Code execution through a file Sentinel reads (a project file, a manifest, a saved record).
- Anything that makes the quarantine, restore or purge act on the wrong items.

How these are meant to be prevented is in [docs/THREAT_MODEL.md](docs/THREAT_MODEL.md) and
[docs/SAFETY_MODEL.md](docs/SAFETY_MODEL.md). If you find a threat row that does not hold, that
is exactly the kind of report wanted. Known limits are listed in
[docs/SECURITY_REVIEW.md](docs/SECURITY_REVIEW.md); reports about those are still welcome but
are already understood.

## Not in scope

- A malicious local administrator, kernel-level attackers, or physical access to the computer.
- Another program running as the same Windows user that edits Sentinel's own files in
  `%LOCALAPPDATA%`: it could already read or delete the same files directly.
- The installer being unsigned (Windows SmartScreen will warn). That is known.

## Supported versions

Only the latest commit on `main` is supported while the project is pre-release.
