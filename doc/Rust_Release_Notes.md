# AudioLib Explorer v0.4.9

Release 0.4.9; the client, CLI and server share the same program version. Patch release on top of the published v0.4.8.

Versions: program 0.4.9 | API /api/v1/ | schema 5 | snapshot compat 3..=5

## Changelog

### Neutral example values and package privacy check

- Example server addresses in the README, CLI help, client UI and tooling now use neutral values such as `https://audiolib.example.com:8443` and `http://server.example.lan:8790`.
- The server admin page's token-name placeholder now uses a neutral example.
- The package privacy check now also rejects packaged files containing the port `18086`, concrete `192.168.x.x` addresses (CIDR ranges such as `192.168.0.0/16` stay allowed) and a standalone personal name, except under `licenses/`.

## Compatibility

API paths, the database schema and the snapshot compatibility range are unchanged, and the settings file format is unchanged. Upgrading keeps libraries, Metadata, annotations, scan history and preferences; runtime state stays in `%LOCALAPPDATA%/AudioLibExplorer/v1` or the existing explicit data directory. The Windows x64 portable package includes WASAPI and ASIO.

The Release provides the Windows x64 client portable package and Windows x64 and Linux x64 standalone server packages, each with a SHA-256 file and an allowed-field validation report.
