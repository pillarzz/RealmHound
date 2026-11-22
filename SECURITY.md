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

`RealmHound` stays unprivileged and capability-free. The installer grants
`cap_net_raw=ep` only to the separately installed helper at
`/usr/local/libexec/realmhound/realmhound-capture`; the helper never uses
`cap_net_admin` or promiscuous mode. The helper directory is `root:root 0755`,
and the helper itself is `root:root 0755` with exactly `cap_net_raw=ep`.

Routine in-app updates replace only the user-owned GUI under
`~/.local/opt/realmhound`. They never modify the helper or apply capabilities.
An older helper that remains protocol-compatible keeps working across GUI
updates.

The routine Linux update artifact is a single executable, not an archive. The
GUI verifies its SHA-256 while downloading into a private file beside the
installation and bounds the download size. It never extracts an archive or
executes an installer. After the hash matches, it may run the downloaded GUI
with `--version` to confirm this host can execute it; that opens no window and
performs no installation. There is therefore no archive path-traversal or
symlink boundary in the unprivileged update path. The full tarball is used only
for manual installation and helper upgrades.

Compatibility is defined by the helper stream protocol and a minimum safe
helper version, not by matching release numbers. Helpers published before the
version handshake are treated as protocol v1 and remain supported. The GUI asks
a helper for its version by running `realmhound-capture --version` as a separate
process; this never changes the packet framing or the helper's stdin/stdout
contract. A release that requires a newer helper publishes that minimum in the
update manifest, and the GUI refuses to replace itself until the helper has been
upgraded and rechecked.

### Manual helper upgrades

Linux file capabilities belong to the inode. Replacing the helper does not
transfer the previous file's capability, and modifying the executable can clear
it. Restoring a host file capability needs `CAP_SETFCAP`, which the GUI and the
helper are not granted. Helpers therefore need a rare manual administrative
upgrade after a security or capture-stack fix.

`packaging/linux/install-helper.sh` is the only privileged helper installer.
Run it with sudo, an official SHA-256, and a helper obtained from an official
release:

```sh
sudo ./packaging/linux/install-helper.sh --sha256 <official helper hash> /path/to/realmhound-capture
```

The digest is published as `RealmHound-linux-x86_64-helper.sha256` with each
release. The installer rejects symlinks, copies into a private root-owned
temporary file in the protected destination directory, verifies the copied
bytes rather than the caller's file, renames the verified inode into place,
applies exactly `cap_net_raw=ep`, and verifies the result. It never installs or launches the
GUI. Because the destination directory is root-owned, unprivileged users
cannot replace the helper.

SHA-256 verifies integrity against expected release metadata; it does not by
itself establish publisher authenticity. Windows, macOS, and Linux trust the
same update manifest, so release signing is a cross-platform concern rather
than something Linux hashing alone solves.

### Shared machines

File capabilities cannot be scoped to one user. Any local account that can
execute the helper can capture the machine's port-2050 traffic. This is accepted
for a single-user desktop and is unchanged by the user-owned GUI migration. To
restrict it on a shared machine, scope execution with a dedicated group
(`chgrp realmhound`, `chmod 0750`) or move capture behind a system service.
