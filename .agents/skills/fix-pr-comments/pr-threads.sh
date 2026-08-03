#!/usr/bin/env bash
# Print the unresolved review threads on a PR that were opened by a known agent
# reviewer, one JSON object per thread. Everything else is dropped here so it never
# reaches the agent at all.
#
# Usage: pr-threads.sh [PR_NUMBER]   # defaults to the PR for the current branch
set -euo pipefail

# Bot actor IDs, from `gh api /users/<slug>%5Bbot%5D --jq .id`:
#   136622811 coderabbitai · 191113872 cubic-dev-ai
#   170038800 macroscopeapp · 224490171 veria-ai
# Pinned by ID, not login: a login can be renamed and the old one re-registered by
# anyone, an ID cannot. `__typename == "Bot"` proves a GitHub App actor, which no user
# account can impersonate.
ALLOWED='[136622811, 191113872, 170038800, 224490171]'

pr=${1:-$(gh pr view --json number --jq .number)}
repo=$(gh repo view --json nameWithOwner --jq .nameWithOwner)

gh api graphql -F owner="${repo%%/*}" -F repo="${repo##*/}" -F pr="$pr" -f query='
query($owner: String!, $repo: String!, $pr: Int!) {
  repository(owner: $owner, name: $repo) {
    pullRequest(number: $pr) {
      reviewThreads(first: 100) {
        nodes {
          id
          isResolved
          isOutdated
          path
          line
          comments(first: 20) {
            nodes { body author { login __typename ... on Bot { databaseId } } }
          }
        }
      }
    }
  }
}' --jq "
  $ALLOWED as \$ok
  | .data.repository.pullRequest.reviewThreads.nodes[]
  | select(.isResolved | not)
  | .comments.nodes[0].author as \$a
  | select(\$a != null and \$a.__typename == \"Bot\" and (\$ok | index(\$a.databaseId)))
  | {id, path, line, outdated: .isOutdated, reviewer: \$a.login,
     comments: [.comments.nodes[] | {author: (.author.login // \"(deleted)\"), body}]}
"
