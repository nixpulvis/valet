# Valet

Valet is a local-first, multi-user password manager. The core library handles
encryption (AES-GCM-SIV + Argon2 key derivation), and multiple clients consume
it:

- **CLI** (`src/bin/cli.rs`): Clap-based REPL for registering users, managing
  lots, and storing/retrieving secrets from the terminal.
- **GUI** (`src/bin/gui/`): Native desktop app built with egui/eframe (behind
  the `gui` feature flag).
- **Browser extension** (`platform/browser/`): browser popup that matches
  credentials to the active tab's domain, with a native messaging host for
  database access. See the [browser extension README](platform/browser/README.md)
  for build and development instructions.

## What Valet Aims to Solve

Most people end up choosing between hosted password managers like 1Password
or Bitwarden, which require trusting a vendor's servers and keeping a
subscription current, and local-first tools like KeePass or `pass`, which
keep you in control of the data but scatter the experience across a loose
collection of third-party clients with inconsistent platform support and
dated cryptography.

Valet is closest in spirit to KeePass: a local, encrypted database that you
own, synced between your own devices rather than through a vendor. The aim is
to take that model and modernize it:

- **Real multi-user, without a shared master password.** A single database
  holds multiple users, each with their own password. Lots (named
  collections of secrets) are shared by re-encrypting the lot key for each
  recipient, so access can be granted and revoked per-user instead of handing
  out one password that everybody has to keep rotating.
- **Durability over destructiveness.** Records store their history and are 
  archived rather than hard-deleted, so an accidental overwrite does not strand
  a working credential.
- **Modern cryptography.** AES-GCM-SIV for nonce-misuse resistance, Argon2
  for key derivation, and AAD binding every ciphertext to the identifier it
  belongs to.
- **Local ownership by default.** The database is a SQLite file with embedded
  Git repos on your disk. Nothing is sent to a vendor by default; there is no
  account to create. Sync is peer-to-peer between your own devices or servers
  over any URL `git` can speak (ssh, https, file).
- **First-party clients on every platform.** The core library is consumed by
  a CLI, a desktop GUI, browser extensions for Firefox/Chrome/Safari, and
  system-level autofill on macOS and iOS, all developed in this repo against
  the same encrypted store.

## Feature Matrix

| Feature                    | CLI | GUI | Firefox | Chrome | Safari | macOS Ext | macOS App | iOS App |
|----------------------------|:---:|:---:|:-------:|:------:|:------:|:---------:|:---------:|:-------:|
| Register user              | 🟩  | 🟩  |   🟦    |  🟦    |  🟦    |    🟥     |    🟦     |   🟦    |
| Unlock / lock              | 🟩  | 🟩  |   🟩    |  🟦    |  🟦    |    🟩     |    🟦     |   🟦    |
| List records               | 🟩  | 🟩  |   🟩    |  🟦    |  🟦    |    🟩     |    🟦     |   🟦    |
| Get / copy password        | 🟩  | 🟩  |   🟩    |  🟦    |  🟦    |    🟩     |    🟦     |   🟦    |
| Add record                 | 🟩  | 🟩  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Edit record                | 🟩  | 🟦  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Archive record             | 🟩  | 🟩  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Multi-user                 | 🟩  | 🟩  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Multi-lot (sharing)        | 🟩  | 🟩  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Auto-fill                  | 🟥  | 🟥  |   🟩    |  🟦    |  🟦    |    🟩     |    🟥     |   🟦    |
| Password generator         | 🟦  | 🟦  |   🟦    |  🟦    |  🟦    |    🟥     |    🟦     |   🟦    |
| SSH / GPG key storage      | 🟦  | 🟦  |   🟥    |  🟥    |  🟥    |    🟥     |    🟦     |   🟥    |
| Passkey provider           | 🟥  | 🟥  |   🟦    |  🟦    |  🟦    |    🟦     |    🟥     |   🟦    |
| Passkey sync               | 🟦  | 🟦  |   🟦    |  🟦    |  🟦    |    🟥     |    🟦     |   🟦    |
| Biometric unlock           | 🟥  | 🟥  |   🟥    |  🟥    |  🟥    |    🟦     |    🟦     |   🟦    |
| Auto-lock on idle          | 🟦  | 🟦  |   🟩    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Import / export            | 🟦  | 🟦  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Offline use                | 🟩  | 🟩  |   🟩    |  🟦    |  🟦    |    🟩     |    🟦     |   🟦    |
| Sync between devices       | 🟦  | 🟦  |   🟦    |  🟦    |  🟦    |    🟦     |    🟦     |   🟦    |
| Daemon-backed (`valetd`)   | 🟦  | 🟦  |   🟩    |  🟦    |  🟦    |    🟩     |    🟦     |   🟥    |

Legend: 🟩 = Supported, 🟦 = Planned, 🟥 = Not supported.

Note: macOS Ext. and the macOS App will be merged before release.

## CLI

A single user pushing one lot to a bare git repo on a server, as an
encrypted offsite backup.

```sh
# one-time, on the server:
$ ssh you@host 'git init --bare ~/valet/main.git'

$ valet register alice
Password: ********
$ valet unlock alice
Password: ********
valet> add github.com/alice s3cret
valet> get github.com/alice
s3cret
valet> remote add origin ssh://you@host/~/valet/main.git main
valet> sync main
main/origin: clean (0 advanced), pushed
```

Each subsequent `sync main` pushes new commits to the remote. The remote
holds only ciphertext; the lot key never leaves the local database. `sync`
with no arguments syncs every lot the user can see.

Restoring on a new machine, and syncing the same lot across two devices,
both require sharing the lot key with another database; that lands with
cross-user lot sharing (see [Threat Models](#threat-models)).

TODO: SSH Keys
TODO: GPG Keys

### Threat Models

As part of designing a local-first, multi-user, distributed password manager,
there are many different threat models to consider at different levels.

##### Single User, Offline
The simplest base threat model involves only a single _user_ operating on a
single _database_ with possibly many _client_ programs (i.e. CLI and GUI). In
this model, the database remains completely under the stewardship of the user.

A corrupted database may no longer work as expected, but the security of the
secrets should remain intact. The only way to leak the secrets would be to
either A) leak the master user password, B) leak the AES key material, or C)
leak the secrets themselves.

Users are free to, and encouraged to maintain a backup, which Valet may assist
with, however, that copy of the database is outside the perview of the
application. There are no syncronization concerns because backups in this model
are read-only snapshots of the current state of the application. Restoring from
an offline backup is either as simple as copying it to the user's primary
database location, or manually copying spesific secrets.

If the client's host's `root` user or kernel is compromised then the security of
the application cannot be ensured. Valet will try to mitigate the leakage, but
the user password, encryption keys, and secrets will be availible in memory.
Malicious user-level programs should not be able to read secrets, thanks to OS
process isolation.

Valet clients will also take measures to avoid losing or leaking information
through careless user interactions. For example, an inactive client should
become locked automatically and secret information should never be displayed
without an explicit request by the user. Any OS integrations (e.g. autofill)
should take care to consider the complete UX of both secret creation and use.
Full history of secrets should be kept by default in case users accidentally
update a secret before confirming it was accepted by it's intended recipient.
Losing a password can be just as bad as having a password stolen.

##### Single User, Online
The next layer adds sync. A single user runs Valet on multiple devices, each
with its own copy of the database, and points each lot at one or more remote
git URLs (ssh, https, or file). `sync` fetches commits from each remote, merges
them into the local lot, and pushes the merged result back. The remote does not
need to be a Valet process; any git endpoint works.

Everything that crosses the wire is ciphertext. A lot's records are encrypted
under the lot key, which the remote never sees, and AAD binds each ciphertext
to the lot uuid plus the record uuid. A malicious remote (or anyone observing
the transport) therefore cannot read or modify record contents, and cannot
substitute one record's ciphertext for another's without the merge failing
authentication.

What a hostile remote *can* do is withhold or replay commits: serve an old
view, drop incoming commits, or hand different clients divergent histories.
Valet keeps full record history so an unwanted overwrite is recoverable, but
freshness is not authenticated end-to-end. Treat the remote as an availability
dependency, not a confidentiality one.

Conflict resolution is intentionally a manual user step. When two devices
edit the same record between syncs, `sync` reports the conflicting entries
and stops; the user picks which side wins. This keeps the trust boundary at
the user rather than handing it to a merge heuristic.

##### Multi User
Sharing a lot between two distinct users is not yet supported. Two pieces
need to land together to make it safe:

- **Lot key sharing.** Lot keys are stored encrypted per-user in `user_lots`.
  Granting a key to a second user means re-encrypting it under a key the
  recipient controls; there is no surface for that yet, so cross-user
  sharing through `sync` alone is impossible today.
- **Recipient authentication.** Before a granter encrypts a lot key for a
  recipient, the granter needs to verify the recipient's public key
  actually belongs to the person they intend to share with. Without an
  identity check, key sharing degrades to trust-on-first-use against
  whoever happens to publish a key under a given name. Valet does not yet
  ship an identity surface for this; it lands alongside key sharing.

##### Single/Multi User, Hosted
TODO: Now we introduce a hosted Valet server, which allows for online
registration. These environments are fundamentally multi-user, since they
necessitate syncing the hosted database with the local client's database.
