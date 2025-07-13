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
