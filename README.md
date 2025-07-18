# Valet

An idea for the CLI...
```sh
$ valet register <username>
valet> Password: <password>

$ valet validate <username>
valet> Password: <password>

$ valet unlock <username>
valet> Password: <password>

valet> add <label> <value>...

valet> del <label>

valet> list
- <label>
- ...

valet> get <label>
<value>...
```

Example Adding Password
```sh
$ valet unlock nixpulvis
valet> Password: mastersecret
valet> add github.com/nixpulvis anothersecret
✅

# Or in oneline (two with the password prompt).
$ valet add github.com/nixpulvis anothersecret
valet> Password: mastersecret
✅
```

Example Getting Password
```sh
$ valet unlock nixpulvis
valet> Password: mastersecret
valet> get github.com/nixpulvis
anothersecret

# Or in oneline (two with the password prompt).
$ valet get github.com/nixpulvis
valet> Password: mastersecret
anothersecret
````

Example Adding SSH Key
```sh
$ valet unlock nixpulvis
valet> Password: mastersecret
valet> add ssh/nixpulvis/machine "..."
valet> add ssh/nixpulvis/machine.pub "..."
✅

# Helper commands
valet> add-ssh ssh/nixpulvis ~/.ssh/machine.pub
```

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

##### Single/Multi User, Online
TODO: Next is a single user with many databases all syncronized manually. Here
databases are transfered between clients with changes merged and conflicts
handled by the user without an active 3rd party. The way the databases are
transfered shouldn't effect the application, however if using a network drive,
each client would still need it's own copy. Here we need to worry about
maliciously corrupted databases trying to steal data through the syncronization
process.

##### Single/Multi User, Hosted
TODO: Now we introduce a hosted Valet server, which allows for online
registration. These environments are fundamentally multi-user, since they
necessitate syncing the hosted database with the local client's database.
