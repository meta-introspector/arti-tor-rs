# `arti hsc`

`arti hsc` is a command line utility for managing client keys. In the future, we
plan to extend it to support managing other types of state as well.

Like the other `arti` subcommands, it has an optional `--config` option for
specifying the TOML configuration file. Using the correct configuration file is
important, because the state and keys managed by `arti hsc` are relative to the
state directory, which you might have overridden in the configuration.

> `arti hsc` is an experimental subcommand.
> To use it, you will need to compile `arti` with the experimental `hsc` feature

## Generating a service discovery key

Client service discovery keys (previously known as "client authorization" keys)
can be generated and/or retrieved using the `arti hsc key get` command.

`key get` prompts the user for an onion address (`<SVC>.onion`). If no keypair
exists for that service, it will first be generated. It then outputs the public
part of that service's key to the file specified with the --output option.

```ignore
$ arti -c hsc.toml hsc key get --output -
Enter an onion address: mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion
descriptor:x25519:RWWKYMW5EXDUZ2ESDDC7FQJCG6ROAR34LXNSTXFSY6JMQOWNDVNQ

```

If you are running this command non-interactively, you can suppress the prompt
with `--batch`:

```ignore
$ echo "mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion" | arti -c hsc.toml hsc key get --output - --batch
descriptor:x25519:RWWKYMW5EXDUZ2ESDDC7FQJCG6ROAR34LXNSTXFSY6JMQOWNDVNQ
```

> NOTE: the public part of the generated keypair must be shared with the
> service, and the service must be configured to allow the client that owns it
> to discover its introduction points. The caller is responsible for sharing the
> public key with the hidden service.

See `arti hsc key get --help` for more information.

## Rotating a service discovery key

Keys can be rotated with the `arti hsc key rotate` command.

To rotate a service discovery key:
```ignore
$ arti -c hsc.toml hsc key rotate --output -
Enter an onion address: mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion
rotate client restricted discovery key for mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion? (type YES or no): YES
descriptor:x25519:4E4B6CILWAAM2JFSVTOTCANCCUIMSOOSXZWONSR52ETXSTCKIYIA
```

> NOTE: if the client keystore already contains a restricted discovery keypair
> for the service, it will be overwritten. Otherwise, a new keypair is generated.

As key rotation is a destructive operation (the old key will be lost),
`arti hsc key rotate` will prompt you to confirm the operation.
If you wish to force removal, or to run this command non-interactively,
use the `-f` option, which disables the confirmation prompt.

> NOTE: as with `arti gsc key get`, the public part of the new keypair
> must be shared with the service

See `arti hsc key rotate --help` for more information.

## Removing a service discovery key

Keys can be rotated with the `arti hsc key remove` command.

To remove a service discovery key:
```ignore
$ arti -c hsc.toml hsc key remove
Enter an onion address: mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion
remove client restricted discovery key for mnyizjj7m3hpcr7i5afph3zt7maa65johyu2ruis6z7cmnjmaj3h6tad.onion? (type YES or no): YES
```

As with `hsc key rotate`, you can disable the confirmation prompt and force
removal using the `-f` option.

See `arti hsc key remove --help` for more information.

## Migrate service discovery keys from CTor to Arti

Service discovery keys contained in the registered CTor keystore can be migrated
to Arti using the command `hsc ctor-migrate`.

```ignore
$ arti -c hsc.toml hsc ctor-migrate
```

The command detects if keys for the services corresponding to the CTor keys are
already present in the primary keystore. If so, the user will be prompted about overwriting.

The `ctor-migrate` command is capable of detecting if a clash occurs (multiple
keys in the registered CTor keystore belonging to the same service). In such cases,
the user is warned, and migration proceeds using only one of the keys. The operation
is safe, as the original CTor keystore remains intact.

You can disable the confirmation prompt and force overwriting using the `-b`
option.

See `arti hsc ctor-migrate --help` for more information.
