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
