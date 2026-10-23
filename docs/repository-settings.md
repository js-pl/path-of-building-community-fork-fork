# Repository settings

GitHub settings are not applied from files, so they have to be set once by
hand. The ruleset is kept in `.github/rulesets/main.json` so it is versioned.

## Ruleset for `main`

Settings → Rules → Rulesets → New ruleset → Import a ruleset →
`.github/rulesets/main.json`.

It applies to the default branch and:

- blocks deleting the branch,
- blocks force pushes (no history rewrites),
- requires linear history,
- requires a pull request for every change (no direct pushes) and only allows
  squash merges.

There are no bypass actors, so the owner cannot push directly either. No
approving review is required, because GitHub does not let authors approve their
own pull requests and this is a single-maintainer repository.

## Merge button

Settings → General → Pull Requests:

- Allow squash merging: on
- Allow merge commits: off
- Allow rebase merging: off
- Automatically delete head branches: on

## Who can merge

Only accounts with write access can merge. Keep js-pl as the only
collaborator (Settings → Collaborators). Pull requests from forks can be opened
by anyone but not merged.
