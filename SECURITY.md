# Security policy

## Supported scope

Security fixes are applied to the latest revision on `main`. This pre-1.0 project does not promise a
stable API or production-grade isolation.

## Reporting

Do not publish credentials, private repository content, or a working exploit in a public issue. Use
GitHub's private vulnerability reporting for this repository. Non-sensitive hardening suggestions
can be filed as ordinary issues.

## Important boundaries

The default executor is not an OS sandbox. Allowlisting, path containment, timeout, output limits,
and process termination reduce the available surface but do not remove the host permissions of an
allowed program. The opt-in macOS Seatbelt backend is the only isolation backend currently included;
no equivalent Linux isolation claim is made.
