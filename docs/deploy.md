# Automatic deploys

`.github/workflows/deploy.yml` deploys `main` to production on every push
(merged pull requests included). In the server's checkout it runs
`git pull` (fast-forward only), `docker compose build` and
`docker compose up -d`. It builds before it restarts, so a broken build
leaves the running version up.

The `testing` branch is not deployed anywhere: pull it and run it locally
with `docker compose up -d --build`. Changes reach `main` through a pull
request.

You can also start a deploy by hand: Actions > Deploy > Run workflow.

Nothing about the server is stored in the repo; it all lives in the
`production` GitHub environment.

## One-time setup

### 1. A key for GitHub Actions

On your own machine, make a key with no passphrase that is only used for
deploys:

```sh
ssh-keygen -t ed25519 -N '' -C ainki-deploy -f ainki-deploy
```

Add it to the deploy user on the server, and record the server's host key so
Actions can tell it is really your server:

```sh
ssh-copy-id -i ainki-deploy.pub user@your.server
ssh-keyscan -t ed25519 your.server > known_hosts
```

Check that `ssh -i ainki-deploy user@your.server` logs in, and that this user
can run `git pull` and `docker compose` in the checkout.

### 2. The production environment

Repository Settings > Environments > New environment, named `production`.

Deployment branches and tags: "Selected branches", add `main`. Only
workflows running on `main` can then read these secrets.

Environment secrets:

- `SSH_HOST`: server hostname or IP
- `SSH_USER`: the deploy user
- `SSH_PRIVATE_KEY`: the whole content of `ainki-deploy`
- `SSH_KNOWN_HOSTS`: the whole content of `known_hosts`

Environment variables:

- `DEPLOY_PATH`: the checkout on the server; `~/...` is relative to the
  deploy user's home
- `SSH_PORT`: only if SSH is not on port 22

Then delete the local `ainki-deploy` private key, or keep it somewhere safe.

### 3. Protect main

Settings > Rules > Rulesets > New branch ruleset, target `main`:

- Require a pull request before merging
- Block force pushes
- Restrict deletions

Only people with write access can merge, so as long as you are the only
collaborator, nothing reaches production without you. If you add
collaborators later, also set "Required approvals" to 1. GitHub doesn't let
you approve your own pull request, so add yourself to the bypass list if you
want to keep merging your own.

## Upgrading from v1 to v2

v2 adds migrations 0009 to 0014. They run by themselves when the new
version starts. They only go forward: v1 refuses to start on a database
that has migrations it does not know. So take a backup first; going back
means restoring it.

### Before the merge

In the server's checkout:

```sh
# Keep the v1 image, so going back needs no rebuild.
docker tag ainki:latest ainki:v1
# A consistent copy of ainki.db and media/.
docker compose stop
docker run --rm -v ainki_ainki-data:/data -v "$PWD":/backup debian:trixie-slim \
  tar czf /backup/ainki-v1-$(date +%F).tgz -C /data .
docker compose start
```

Copy the tarball off the server as well. While you are there, check that
`docker-compose.override.yml` does not set `AINKI_ALLOW_SIGNUP=true`:
v2 makes signup invite-only only while it is false.

To rehearse, unpack a copy on your own machine and start v2 on it with
`AINKI_DATA_DIR=<the copy> cargo run --release`.

### The merge

Merge v2 into `main` with a merge commit (not squash or rebase), so later v2
work still merges cleanly. The push deploys as usual.

### After the deploy

1. Settings shows the new build; `docker compose logs ainki` has no errors.
2. Create the admin account once:
   `docker compose exec ainki ainki create-admin admin 'a-long-password'`.
3. Study a few cards, open Stats, and open an invite link in a private
   window.

### Going back

Reviews made on v2 since the backup are lost.

```sh
docker compose stop
docker run --rm -v ainki_ainki-data:/data -v "$PWD":/backup debian:trixie-slim \
  sh -c 'rm -rf /data/* && tar xzf /backup/ainki-v1-<date>.tgz -C /data'
git checkout --detach <last v1 commit>
docker tag ainki:v1 ainki:latest
docker compose up -d --no-build
```

Then revert the merge on `main`, or the next deploy brings v2 back. The
deploy's own `git checkout main` returns the checkout to the branch.
