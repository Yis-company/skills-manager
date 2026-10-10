# Credentials for a headless Linux host to pull and push a private GitHub repo

Research for Linear SID-96. Researched 2026-10-10 against GitHub Docs, the Git documentation, OpenSSH manual pages, and this repository's source. This note presents facts and trade-offs; it does not make the "host syncs itself" vs "Mac relays" decision.

Labels: **[doc]** is stated by the cited primary source. **[code]** is verified in this repo. **[inference]** is my reasoning from those facts.

## Short answer

- **Deploy key (SSH, one per host)** is the only option that works with the Mac offline and holds no expiring secret. It is limited to one repository and needs no GitHub user identity. It also never expires, and the private key usually sits unencrypted on the host.
- **Fine-grained PAT** can be limited to the one repository with `Contents: write`, and it can expire. It must be created by a person in the GitHub web UI. On the host it is a bearer token, and without a secret service it ends up in a plaintext file.
- **GitHub App installation tokens** are the narrowest and shortest-lived (1 hour). To refresh without the Mac, the host has to hold the App's private key, which never expires and can mint tokens for the whole installation.
- **SSH agent forwarding** stores nothing on the host. It works only while the Mac holds an open SSH session, and while that session is open it exposes every repository the user's key can reach.
- **Credential helpers** are storage, not credentials. On Linux with no secret service, the persistent built-in choice is a plaintext file with mode 0600 (`store`). The alternatives are in-memory and lost on reboot (`cache`, kernel keyutils) or need GPG plus a TTY (GCM `gpg`).
- **Mac relays** avoids every GitHub credential on the host. The cost is that the host syncs only when the Mac is online.

## What the app does today

- The backup remote is HTTPS. The Mac authenticates with a PAT or an OAuth device-flow token. The device flow requests the classic `repo` scope through an OAuth App client id (`src-tauri/src/core/github_api.rs:19`, `:187`). **[code]**
- Tokens go to the OS keychain through the `keyring` crate. On Linux the only backend compiled in is `sync-secret-service`, which uses D-Bus (`src-tauri/Cargo.toml:53`). **[code]**
- When the keychain is unavailable, `sanitize_url_to_keychain` keeps the credentials in the remote URL (`src-tauri/src/commands/git_backup.rs:85-105`). Guided GitHub connect refuses that fallback (`git_backup.rs:122-126`). **[code]**
- System git receives the stored credential through a static `GIT_ASKPASS` script and environment variables (`src-tauri/src/core/git_credentials.rs:237-268`). The git2 engine handles only `http(s)` URLs, and its callbacks offer only username and password (`src-tauri/src/core/git2_engine.rs:46-72`). An `ssh://` or `git@github.com:` backup remote therefore takes the system `git` path (`src-tauri/src/core/git_backup.rs:12-21`). That path needs `git` and `ssh` installed on the host. **[code]**
- The app reaches hosts with `ssh -T -o BatchMode=yes -o ConnectTimeout=10 … <target> sh -c …` (`src-tauri/src/core/remote_host.rs:37-54`). It does not pass `-A` or `ForwardAgent`. **[code]** `-T` disables pseudo-terminal allocation, and `SSH_TTY` is set only for a tty ([ssh(1)](https://man.openbsd.org/ssh)). **[doc]** So any host-side passphrase or PIN prompt has no terminal over this channel. **[inference]**

## Cross-cutting facts for any SSH-to-GitHub option

- `BatchMode=yes` disables host-key confirmation prompts ([ssh_config(5)](https://man.openbsd.org/ssh_config)). **[doc]** A first unattended `git` to `github.com` from the host therefore fails unless `known_hosts` already has GitHub's key. The app can write GitHub's published entries ([GitHub's SSH key fingerprints](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints)) or use `StrictHostKeyChecking=accept-new`, which trusts the key on first use ([ssh_config(5)](https://man.openbsd.org/ssh_config)). **[doc]**
- A key can be pinned per repository with `core.sshCommand` (for example `ssh -i <key> -o IdentitiesOnly=yes`). `git fetch` and `git push` then use that command instead of `ssh` ([git-config core.sshCommand](https://git-scm.com/docs/git-config#Documentation/git-config.txt-coresshCommand); [ssh_config IdentitiesOnly](https://man.openbsd.org/ssh_config)). **[doc]**
- If outbound port 22 is blocked, GitHub serves SSH at `ssh.github.com:443` ([Using SSH over the HTTPS port](https://docs.github.com/en/authentication/troubleshooting-ssh/using-ssh-over-the-https-port)). **[doc]**
- Anything passed as an ssh command argument becomes part of the remote command line. The [`transfer.credentialsInUrl` docs](https://git-scm.com/docs/git-config#Documentation/git-config.txt-transfercredentialsInUrl) warn that URLs given as arguments are visible to other users in the process list. **[doc]** Secrets sent from the Mac should therefore travel on ssh stdin, not argv. **[inference]**

## Options

### 1. Deploy key

- **Setup from the app over SSH:**
  1. On the host, run `ssh-keygen -t ed25519 -N "" -f <path>` and read back the `.pub` file. The private key never leaves the host.
  2. The Mac registers the public key with `POST /repos/{owner}/{repo}/keys` and `read_only: false` ([REST deploy keys](https://docs.github.com/en/rest/deploy-keys/deploy-keys)). For a fine-grained token, this endpoint needs `Administration: write` ([permissions for fine-grained PATs](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens)). **[doc]** The docs do not say whether the app's classic `repo`-scoped OAuth token can create deploy keys; see Open questions. The user can also paste the key under repository Settings > Deploy keys ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)).
  3. On the host, add the `known_hosts` entries, set the remote to `git@github.com:owner/repo.git`, and set `core.sshCommand`.
- **Scope:** a single repository. "A deploy key cannot be reused across repositories" ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]** A repository can hold several deploy keys, so each host can have its own. **[inference]**
- **Read/write:** read-only by default. Write is enabled when the key is added. A write key "can perform the same actions as an organization member with admin access, or a collaborator on a personal repository" ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]**
- **Expiry/rotation:** "Deploy keys are credentials that don't have an expiry date." Keys are immutable, so rotation means deleting the key and creating a new one ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys); [REST](https://docs.github.com/en/rest/deploy-keys/deploy-keys)). **[doc]**
- **Revocation:** delete the key in repository settings or with `DELETE /repos/{owner}/{repo}/keys/{key_id}`. If a fine-grained PAT created the deploy key, deleting that token also deletes the key ([Managing PATs](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]** The key is tied to the repository, not a user, so it survives the creator losing access ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]**
- **Storage on host:** a private key file. GitHub notes deploy keys "are usually not protected by a passphrase" ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]** A passphrase would need an unlocked agent on the host, which an unattended host lacks. **[inference]**
- **Mac offline:** works.
- **Risks:** anyone who can read the key file can push to that one repository indefinitely. Nothing about it expires, so a forgotten host keeps working until someone deletes the key.

### 2. Fine-grained personal access token

- **Setup from the app over SSH:**
  1. There is no REST API that creates PATs. A person creates the token in the web UI ([Managing PATs](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]**
  2. The Mac can open a pre-filled URL such as `https://github.com/settings/personal-access-tokens/new?name=…&expires_in=…&contents=write`. The documented pre-fill parameters do not include repository selection ([same page](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]**
  3. The user pastes the token into the app. The app sends it to the host over ssh stdin, and the host stores it (see option 5). Git takes it as the HTTPS password. The username is required but not used, and tokens work only over HTTPS ([same page](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]**
- **Scope:** "Only select repositories" of one resource owner. Tokens also always read all public repositories, and one token cannot span several organizations ([same page](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]**
- **Read/write:** `Contents` read or write. `Metadata` is read-only ([same page](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens)). **[doc]**
- **Expiry/rotation:** set at creation, from 1 to 366 days or `none`. The default is 30 days, and an organization or enterprise policy can cap it. "Upon reaching your token's expiration date, the token is automatically revoked." Tokens unused for a year are removed ([Managing PATs](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens); [Token expiration and revocation](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/token-expiration-and-revocation)). **[doc]** Rotation is a manual web-UI step each time. **[inference from no create API]**
- **Revocation:** Settings > Developer settings > Fine-grained tokens > Delete. A token pushed to a public repository or gist is revoked automatically. A token becomes inactive if its user loses access to the resource ([Managing PATs](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens); [Token expiration and revocation](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/token-expiration-and-revocation)). **[doc]** One token per host lets one host be revoked alone. **[inference]**
- **Storage on host:** a bearer secret. Without a secret service it lands in a plaintext file or in memory (option 5).
- **Mac offline:** works until the token expires.
- **Risks:** a stolen token works from anywhere until it expires or is deleted. It acts as the user, limited to the selected repository and permissions.

### 3. GitHub App installation access token

- **Setup:**
  1. Someone registers a GitHub App with `Contents: write` and installs it on "Only select repositories", choosing the backup repository ([Installing your own GitHub App](https://docs.github.com/en/apps/using-github-apps/installing-your-own-github-app); [Choosing permissions](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app)). **[doc]**
  2. The manifest flow can pre-configure the registration from a browser on the Mac. Its conversion step returns the App id and the private key PEM, and the flow must finish within one hour ([Registering from a manifest](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest)). **[doc]**
  3. Minting a token: sign a JWT with the App private key, then call `POST /app/installations/{id}/access_tokens`. The body can narrow `repositories` and `permissions` ([Authenticating as an installation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)). **[doc]**
  4. Git uses `x-access-token` as the username and the token as the password ([same page](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)). **[doc]**
- **Two ways to deploy it, with different properties [inference]:**
  - **(a) The Mac mints and hands over a 1-hour token per sync, over ssh stdin.** The host keeps no long-lived secret, but this needs the Mac online, so it behaves like relaying.
  - **(b) The host keeps the App private key and mints its own tokens.** This works offline, but the host then holds a secret that never expires. If the App is shared, its key mints tokens for every installation, so an open-source app cannot ship one key to users' hosts. Each user would register their own App, which is what the manifest flow is for.
- **Scope:** only repositories inside the installation, narrowed per token, up to 500 ([Authenticating as an installation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)). **[doc]**
- **Read/write:** per permission. `Contents: write` allows HTTPS push. **[doc, push implied]**
- **Expiry/rotation:** tokens expire after 1 hour ([Authenticating as an installation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)). "Private keys do not expire and instead need to be manually revoked." Up to 25 keys can exist at once for rotation ([Managing private keys](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps)). **[doc]**
- **Revocation:** for a token, `DELETE /installation/token` ([REST installations](https://docs.github.com/en/rest/apps/installations)) or waiting for expiry. Wider levers are removing the repository from the installation, uninstalling, or deleting the private key ([Managing private keys](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps)). **[doc]**
- **Storage on host:** in (a), a short-lived token in memory or a 0600 file. In (b), a PEM file. GitHub ranks a key vault strongest and an environment variable weaker, and says never to hard-code the key ([Managing private keys](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps)). **[doc]**
- **Mac offline:** (a) no, (b) yes.
- **Risks:** this option needs the most new machinery (App registration, JWT signing, minting). In (b), a leaked PEM is a permanent credential for the installation.
- **Related variant:** GitHub App *user* access tokens from the device flow expire after 8 hours. Their refresh token (`ghr_`) lasts 6 months, so the host would store the refresh token. These tokens act with the intersection of what the user and the App can do ([User access tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app)). **[doc]**

### 4. SSH agent forwarding

- **Setup from the app:**
  1. Add `-o ForwardAgent=yes` to the app's ssh invocation. Today it is not set (`remote_host.rs:37-54`). **[code]**
  2. The Mac's agent must hold a GitHub-authorized key (`ssh-add -L`) ([Using SSH agent forwarding](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/using-ssh-agent-forwarding)). **[doc]**
  3. The host needs an SSH remote URL (`git@github.com:…`) and GitHub in `known_hosts`. The host's sshd allows forwarding by default (`AllowAgentForwarding` defaults to `yes`, [sshd_config(5)](https://man.openbsd.org/sshd_config)). **[doc]**
- **Scope:** whatever the forwarded key can do. With a user-account key, the server gets "the same permissions … that [the user has] locally" ([Managing deploy keys, agent forwarding section](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]**
- **Read/write:** same as the key.
- **Expiry/rotation:** nothing to rotate on the host. Access exists only while the forwarded connection is open ([Using SSH agent forwarding](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/using-ssh-agent-forwarding)). **[doc]** `ssh-add -t` can cap how long a key stays in the agent ([ssh-add(1)](https://man.openbsd.org/ssh-add)). **[doc]**
- **Revocation:** close the session or remove the key from the agent. The GitHub key itself is revoked in account settings.
- **Storage on host:** none. There is only a forwarded socket (`SSH_AUTH_SOCK`) for the session ([ssh(1)](https://man.openbsd.org/ssh)). **[doc]**
- **Mac offline:** does not work. GitHub lists as a con: "automated deploy processes can't be used" ([Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys)). **[doc]**
- **Risks:** while the session is open, anyone on the host who can bypass file permissions on the agent socket can authenticate with the Mac's keys. The keys cannot be extracted, but they can be used ([ssh(1) -A](https://man.openbsd.org/ssh); [ssh_config ForwardAgent](https://man.openbsd.org/ssh_config)). **[doc]**
- **Mitigations:**
  - `ssh-add -c` requires confirmation on each use, but the confirmation goes through `ssh-askpass` on the Mac ([ssh-add(1)](https://man.openbsd.org/ssh-add)). **[doc]**
  - Destination constraints (`ssh-add -h`, OpenSSH 8.9+) can limit a forwarded key to particular hops. They need the `publickey-hostbound` extension on each server past the first hop, and unsupported servers are refused ([ssh-add(1)](https://man.openbsd.org/ssh-add); [OpenSSH agent restriction](https://www.openssh.org/agent-restrict.html)). **[doc]**

### 5. Git credential helpers on Linux without a secret service

These store an HTTPS token (option 2 or 3). They are not a credential in themselves.

| Helper | Where the secret lives | Survives reboot | Notes |
|---|---|---|---|
| Token in remote URL (today's fallback) | `.git/config`, plaintext | yes | Git's own docs list three risks: the config file may not be permission-restricted, backups can leak it, and the URL shows in the process list when passed as an argument ([`transfer.credentialsInUrl`](https://git-scm.com/docs/git-config#Documentation/git-config.txt-transfercredentialsInUrl)). **[doc]** |
| `store` | `~/.git-credentials` or `$XDG_CONFIG_HOME/git/credentials`, one `https://user:pass@host` line per credential | yes | "store your passwords unencrypted on disk, protected only by filesystem permissions". The file is made unreadable to other users ([git-credential-store](https://git-scm.com/docs/git-credential-store)). **[doc]** |
| `cache` | memory of a daemon, reached by a user-only Unix socket | no | Default lifetime is 900 s. The docs call it "inherently unsuitable for persistent storage of personal access tokens" ([git-credential-cache](https://git-scm.com/docs/git-credential-cache)). **[doc]** |
| GCM `secretservice` | libsecret collection | n/a | "Requires a graphical user interface session" ([GCM credential stores](https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/credstores.md)). **[doc]** |
| GCM `gpg` (`pass`) | GPG-encrypted files in `~/.password-store` | yes | Needs `gpg`, `pass`, a key pair, and a pinentry that can use `SSH_TTY` or `GPG_TTY` ([GCM credential stores](https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/credstores.md)). **[doc]** The app's `ssh -T` provides no TTY. A passphrase-less GPG key on the same disk gives little over plaintext. **[inference]** |
| GCM `plaintext` | files under `~/.gcm/store`, in a directory created with mode 700 | yes | "This is not a secure method of credential storage!" ([GCM credential stores](https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/credstores.md)). **[doc]** |
| Kernel keyutils (`keyring` crate `linux-native`) | kernel memory | no | "will not persist across reboots" ([keyring keyutils docs](https://docs.rs/keyring/3.6.3/keyring/keyutils/index.html)). **[doc]** Not compiled into the app today (`Cargo.toml:53`). **[code]** |
| Custom helper (`credential.helper = !<cmd>`) | wherever the command keeps it | depends | Git calls the helper with `get`, `store`, or `erase`, and credential details travel on stdin and stdout ([gitcredentials](https://git-scm.com/docs/gitcredentials)). **[doc]** The app already injects credentials at call time through `GIT_ASKPASS` (`git_credentials.rs:237-268`). **[code]** |

- **Setup from the app over SSH:**
  1. `git config credential.helper store`
  2. Pipe `protocol=https`, `host=github.com`, `username=…`, and `password=…` on stdin to `git credential approve`, which hands the credential to the configured helpers ([git-credential](https://git-scm.com/docs/git-credential)). **[doc]**
- **Expiring tokens:** helpers can carry `password_expiry_utc`, and `git credential fill` ignores expired passwords ([git-credential](https://git-scm.com/docs/git-credential)). **[doc]**
- **Scope:** by default, helpers match on host only, not path, unless `credential.useHttpPath` is set. A token stored for `github.com` is then offered to every GitHub repository on that host account ([gitcredentials](https://git-scm.com/docs/gitcredentials)). **[doc]**
- **Expiry, revocation, and Mac offline:** inherited from the stored token.
- **Persistence without plaintext on disk:** none of the options above give an unattended headless host both. **[inference from the table]**

## Comparison

| Option | Setup from the app over SSH | Repo scope | Read/write | Expiry / rotation | Revocation | Storage on host | Mac offline | Main risks |
|---|---|---|---|---|---|---|---|---|
| Deploy key | `ssh-keygen` on host, then API or UI to add the public key, plus `known_hosts` and `core.sshCommand` | one repo | read, or read/write if enabled | never expires; delete and recreate to rotate | delete the key (UI or `DELETE …/keys/{id}`) | private key file, usually no passphrase | works | permanent push credential on disk |
| Fine-grained PAT | person creates it in the web UI (pre-fill URL), app pipes it on stdin to a helper | selected repos of one owner | `Contents` read or write | 1–366 days or none; manual renewal | delete in settings; auto-revoked on expiry, a year unused, or a public leak | token file or URL, plaintext | works until expiry | bearer token as the user; renewal needs a person |
| App installation token, Mac mints | App setup once; Mac signs JWT, mints, and pipes the token | repos in the installation, narrowed per token | per permission | 1 hour | `DELETE /installation/token`, uninstall, or delete the key | short-lived token | does not work | needs App machinery |
| App installation token, host mints | as above, plus the PEM copied to the host | as above | as above | token 1 hour; PEM never expires | delete the PEM key, uninstall | PEM file | works | PEM is a permanent credential for the installation |
| SSH agent forwarding | add `ForwardAgent=yes` to the app's ssh; key loaded in the Mac agent | everything the key's user can reach | same as the user | none on host | close the session or remove the key from the agent | nothing persisted | does not work | root on the host can use the Mac's keys while connected |
| Credential helper | `git config credential.helper …` plus `git credential approve` on stdin | inherits the token (host-wide unless `useHttpPath`) | inherits | inherits | inherits | plaintext (`store`), memory (`cache`), or GPG with a TTY | as the token | storage choice only |

## What "Mac relays" would avoid

These facts follow from the options above. **[inference]**

- **On the host:** no GitHub credential of any kind, so nothing to create, store, rotate, or revoke per host. The host needs no `known_hosts` entry for GitHub and no outbound route to github.com. The plaintext-storage problem in option 5 disappears.
- **On GitHub:** no deploy key per host, and no per-host PAT expiry to renew in the web UI.
- **On the Mac:** the existing keychain token never leaves the Mac. Agent forwarding is not needed, so nothing is exposed while connected.
- **What it costs:** the host syncs only when the Mac is online and connected.
- **Transport:** git natively speaks ssh to any host with git installed ([Git URLs](https://git-scm.com/docs/git-fetch#_git_urls)). **[doc]** A Mac-to-host transfer could use the app's existing SSH channel. The design choice is open.

## Open questions

- **Classic `repo` scope and deploy keys:** the docs do not say whether the app's `repo`-scoped OAuth token can call `POST /repos/{owner}/{repo}/keys`. The [scopes page](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps) does not mention deploy keys. Listing deploy keys with a `repo`-scoped token returned HTTP 200 (observed with `gh api`), but creating one was not tested.
- **`publickey-hostbound` on GitHub:** whether GitHub's SSH endpoint supports it, which forwarded destination-constrained keys would need, is not documented in the sources read.
- **Agent socket in the macOS app:** whether the app, when launched from Finder, inherits a working `SSH_AUTH_SOCK` for forwarding was not checked.

## Sources

- GitHub Docs: [Managing deploy keys](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/managing-deploy-keys), [REST deploy keys](https://docs.github.com/en/rest/deploy-keys/deploy-keys), [Permissions required for fine-grained PATs](https://docs.github.com/en/rest/authentication/permissions-required-for-fine-grained-personal-access-tokens), [Managing your personal access tokens](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens), [Token expiration and revocation](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/token-expiration-and-revocation), [Authenticating as a GitHub App installation](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation), [Managing private keys for GitHub Apps](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/managing-private-keys-for-github-apps), [Generating a user access token for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app), [REST installations](https://docs.github.com/en/rest/apps/installations), [Installing your own GitHub App](https://docs.github.com/en/apps/using-github-apps/installing-your-own-github-app), [Choosing permissions for a GitHub App](https://docs.github.com/en/apps/creating-github-apps/registering-a-github-app/choosing-permissions-for-a-github-app), [Registering a GitHub App from a manifest](https://docs.github.com/en/apps/sharing-github-apps/registering-a-github-app-from-a-manifest), [Scopes for OAuth apps](https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps), [Using SSH agent forwarding](https://docs.github.com/en/authentication/connecting-to-github-with-ssh/using-ssh-agent-forwarding), [GitHub's SSH key fingerprints](https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/githubs-ssh-key-fingerprints), [Using SSH over the HTTPS port](https://docs.github.com/en/authentication/troubleshooting-ssh/using-ssh-over-the-https-port)
- Git: [gitcredentials](https://git-scm.com/docs/gitcredentials), [git-credential](https://git-scm.com/docs/git-credential), [git-credential-store](https://git-scm.com/docs/git-credential-store), [git-credential-cache](https://git-scm.com/docs/git-credential-cache), [git-config](https://git-scm.com/docs/git-config) (`transfer.credentialsInUrl`, `core.sshCommand`), [Git URLs](https://git-scm.com/docs/git-fetch#_git_urls)
- Git Credential Manager: [Credential stores](https://github.com/git-ecosystem/git-credential-manager/blob/main/docs/credstores.md)
- OpenSSH: [ssh(1)](https://man.openbsd.org/ssh), [ssh_config(5)](https://man.openbsd.org/ssh_config), [sshd_config(5)](https://man.openbsd.org/sshd_config), [ssh-add(1)](https://man.openbsd.org/ssh-add), [Agent restriction](https://www.openssh.org/agent-restrict.html)
- keyring crate: [crate docs](https://docs.rs/keyring/3.6.3/keyring/), [keyutils module](https://docs.rs/keyring/3.6.3/keyring/keyutils/index.html)
