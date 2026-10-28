# Security Policy

## Supported versions

Only the latest stable RealmHound release receives security fixes.
Prereleases are provided for testing and are not supported.

## Reporting a vulnerability

Do not open a public issue for a suspected vulnerability.

Use GitHub's
[private vulnerability reporting](https://github.com/pillarzz/RealmHound/security/advisories/new)
to describe the issue, affected versions, impact, and reproduction steps.

Do not attach unsanitized packet captures, logs, tokens, account exports, or
other files containing player, account, session, or personal data. Explain
what evidence is available first so a maintainer can arrange a safe way to
review it if necessary.

The maintainer will review the report, request additional information when
needed, and coordinate disclosure after a fix is available.

## Linux capture helper

`packaging/linux/install.sh` grants `cap_net_raw=ep` to
`/usr/local/bin/realmhound-capture` only; `RealmHound` itself stays
unprivileged, and the helper never uses `cap_net_admin` or promiscuous mode.

The helper is installed `root:root 0755` with a file capability, and file
capabilities cannot be scoped to one user. Any local account that can execute
the helper can therefore capture the machine's port-2050 traffic. This is
accepted for a single-user desktop. To restrict it on a shared machine, scope
execution with a dedicated group (`chgrp realmhound`, `chmod 0750`) or move
capture behind a system service.
