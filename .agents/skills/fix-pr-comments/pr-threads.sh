#!/usr/bin/env bash
# Print the unresolved review threads on a PR that were opened by a known agent
# reviewer or a repo maintainer, one JSON object per thread. Everything else is dropped
# here so it never reaches the agent at all.
#
# Usage: pr-threads.sh [PR_NUMBER]   # defaults to the PR for the current branch
set -euo pipefail

pr=${1:-$(gh pr view --json number --jq .number)}
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)

# Maintainers: collaborators with push access, pinned by user ID. `authorAssociation`
# won't do, a read-only org member is still MEMBER. Listing them needs push access.
if ids=$(gh api "/repos/$repo/collaborators?per_page=100" --paginate \
    --jq '.[] | select(.permissions.push) | .id' 2>/dev/null); then
  maintainers=$(printf '%s\n' "$ids" | jq -s .)
else
  echo "warning: cannot list collaborators of $repo, trusting agent reviewers only" >&2
  maintainers='[]'
fi

# Quoted delimiter: no shell expansion, so the jq program is literal.
# Bot actor IDs, from `gh api /users/<slug>%5Bbot%5D --jq .id`:
#   136622811 coderabbitai · 191113872 cubic-dev-ai
#   170038800 macroscopeapp · 224490171 veria-ai
# Pinned by ID, not login: a login can be renamed and the old one re-registered by
# anyone, an ID cannot. `__typename == "Bot"` proves a GitHub App actor, which no user
# account can impersonate. Applied to every comment, not just the thread opener — anyone
# can reply to a bot's thread.
read -r -d '' FILTER <<'JQ' || true
def kind: .author as $a
  | if $a == null then null
    elif $a.__typename == "Bot"
      and ([136622811, 191113872, 170038800, 224490171] | index($a.databaseId) != null)
      then "bot"
    elif $a.__typename == "User" and ($maintainers | index($a.databaseId) != null)
      then "maintainer"
    else null end;

.data.repository.pullRequest.reviewThreads.nodes[]
| select(.isResolved | not)
| select(.comments.nodes[0] | kind != null)
| {
    id, path, line,
    outdated: .isOutdated,
    reviewer: .comments.nodes[0].author.login,
    reviewer_kind: (.comments.nodes[0] | kind),
    comments: [.comments.nodes[] | select(kind != null) | {author: .author.login, kind: kind, body}],
    # Replies from anyone else are withheld, not shown: a PR author can reply to a
    # bot's thread. The count is here only so you know to look at the PR yourself.
    withheld_replies: ([.comments.nodes[] | select(kind == null)] | length),
  }
JQ

# --paginate follows the reviewThreads cursor, so PRs with over 100 threads are not
# silently truncated. Comments are capped at 100 per thread; withheld_replies would show
# if a thread ever ran longer. A separate jq takes the maintainer list as an argument.
gh api graphql --paginate -F owner="${repo%%/*}" -F repo="${repo##*/}" -F pr="$pr" -f query='
query($owner: String!, $repo: String!, $pr: Int!, $endCursor: String) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $pr) {
      reviewThreads(first: 100, after: $endCursor) {
        pageInfo { hasNextPage endCursor }
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 100) {
            nodes {
              body
              author { login __typename ... on Bot { databaseId } ... on User { databaseId } }
            }
          }
        }
      }
    }
  }
}' | jq --argjson maintainers "$maintainers" "$FILTER"
