# Omloom

Infrastructure knowledge and operations.

Inventory contracts, embedded persistence and a local development workspace are
under development. No installable product release is available yet.

## Development

Install Rust, Node.js 24+, Corepack and protoc 36.2+, then run:

```sh
corepack enable
yarn install --immutable
yarn dev
```

`yarn dev` (also `yarn start`) generates the clients, builds the interface and
starts the local development server. Open the printed localhost URL. Inventory
persists in `var/development`; Ctrl+C stops the server. This uses the explicit
test operator, not production authentication. Restart the command after source
changes; live reload is not enabled. See [launch options](docs/inventory.md#run).

See [contract development and verification](docs/contracts.md) for Rust,
TypeScript and browser checks. Documentation checks: `scripts/check-docs.sh`
(requires lychee). These checks do not certify an installable application.
See [embedded storage](docs/storage.md) for the host API, durability boundaries
and current provider limitations.
See [the local inventory workspace](docs/inventory.md) for the explicit fixture
host, browser workflow and its development-only boundary.
The workspace includes [current ownership and responsibility](docs/inventory.md#ownership-and-responsibility),
selected external-account references and durable owner-action requests. These
records do not grant infrastructure or credential access.
It also supports [software installations and workloads](docs/inventory.md#software-and-workloads)
as current inventory, independently of discovery or execution adapters.
Devices can retain [partial equipment knowledge](docs/inventory.md#partly-identified-equipment),
including bounded quantities, model alternatives and supported exclusions,
then be refined without changing their identity.

## License

Copyright © 2026 Dmitry Prudnikov.

Licensed under the GNU Affero General Public License v3.0 only;
see [LICENSE](LICENSE).

Supported by Structured World.

Contributions are accepted under the
[Structured World Contributor License Agreement](https://sw.foundation/cla);
see [CONTRIBUTING.md](CONTRIBUTING.md).
