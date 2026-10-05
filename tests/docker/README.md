# Test servers

Real servers for the `integration` tests of `filecargo-remote-fs` (and later `transfer` and
`terminal`). Everything in this directory is throwaway test material: the keys, certificates
and passwords are committed on purpose and are used nowhere else.

```bash
docker compose -f tests/docker/compose.yml up -d --build --wait
cargo it                      # alias for: cargo test -p filecargo-remote-fs --features integration
docker compose -f tests/docker/compose.yml down
```

On macOS with OrbStack, run `orb start` first.

| Service | Port | Login | Notes |
|---|---|---|---|
| `sftp` | 2222 | `fcuser` / `fcpass`, key `keys/id_plain`, key `keys/id_encrypted` (passphrase `filecargo-test-pass`) | OpenSSH, bash shell |
| `ftp` | 2121 (passive 30000-30009) | `ftpuser` / `ftppass` | plain FTP and explicit FTPS (`AUTH TLS`), `NoSessionReuseRequired` |
| `ftps-implicit` | 9990 (passive 30010-30019) | same | implicit FTPS |
| `ftps-reuse` | 2122 (passive 30020-30029) | same | explicit FTPS, session reuse required (ProFTPD default) |
| `ftp-list` | 2123 (passive 30030-30039) | same | plain FTP with `FactsAdvertise off`: no MLSD in FEAT, forces the `LIST` fallback |

- `certs/server.crt` is self-signed for `localhost` / `127.0.0.1`. `certs/other.crt` is a second
  certificate for the same name, used to test that a changed cert prompts again.
- Active-mode FTP cannot work through published localhost ports; those tests run on Linux CI
  against the container IP.
- Regenerate the material with `ssh-keygen -t ed25519` and
  `openssl req -x509 -newkey rsa:2048 -nodes -days 36500 -subj /CN=localhost -addext subjectAltName=DNS:localhost,IP:127.0.0.1`,
  then rebuild `keys/authorized_keys` from the two `.pub` files.
