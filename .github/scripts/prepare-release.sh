#!/usr/bin/env bash
# Compute the next version and rendered changelog for a development -> master
# release PR. Run from the prepare-release.yml workflow on PR open/sync.
#
# Input environment:
#   BASE_REF   the PR's base ref (e.g. master)
#   HEAD_REF   the PR's head ref (e.g. development)
#   REPO_URL   the repo's https URL, for PR links in the changelog
#
# Output (written to $GITHUB_OUTPUT lines and stdout):
#   bump=major|minor|patch|none
#   old_version=X.Y.Z
#   new_version=X.Y.Z         (same as old_version when bump=none)
#   changelog_path=/tmp/...   path to a file containing the rendered markdown
#
# Side effects: writes the new version into Cargo.toml IF bump != none.
# The caller decides whether to commit + push that change.
#
# Verb rules (commit-message prefix, with optional !/!! markers):
#   feat fix chore perf test deps build  -> patch bump
#   feat! / fix! / etc                    -> patch + minor bump (resets minor on major)
#   feat!! / fix!! / etc                  -> patch + major bump (minor resets to 0)
#   docs ci style                         -> no bump, surfaced in changelog
#   anything else                         -> patch bump, listed under Other
#
# Multiple ! markers in one batch: highest wins, applied once. So 1x feat!
# + 1x fix!! + 3x plain fix = single major bump (+ patch always increments).
#
# Bump commits authored by GitHub Actions are skipped to avoid double-counting
# the workflow's own version-bump commit on subsequent runs.

set -euo pipefail

BASE_REF="${BASE_REF:-master}"
HEAD_REF="${HEAD_REF:-development}"
REPO_URL="${REPO_URL:-}"

# Fetch enough history that BASE_REF..HEAD_REF resolves on a shallow CI clone.
git fetch --no-tags --depth=200 origin "$BASE_REF" "$HEAD_REF" 2>/dev/null || true

# Current version from Cargo.toml's [workspace.package].
old_version=$(grep -m1 '^version' Cargo.toml | sed -E 's/version *= *"([^"]+)".*/\1/')
if ! [[ "$old_version" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
    echo "error: workspace version '$old_version' is not X.Y.Z" >&2
    exit 1
fi
old_major="${BASH_REMATCH[1]}"
old_minor="${BASH_REMATCH[2]}"
old_patch="${BASH_REMATCH[3]}"

# Walk commits in BASE_REF..HEAD_REF, oldest first, skipping merge commits and
# version-bump commits authored by the workflow.
commit_log=$(git log "origin/$BASE_REF..origin/$HEAD_REF" \
    --no-merges \
    --reverse \
    --format='%H%x09%an%x09%s' || true)

# Buckets: each section is a verb -> list of "PR_NUM|subject" lines.
declare -A SECTIONS

# Counters accumulate across the whole batch. Each contributing commit
# advances every counter whose marker it carries. Rules:
#
#   - Any bumping verb commit OR untagged "Other" commit: patch += 1
#   - A `!` marker on a bumping verb:                     minor += 1
#   - A `!!` marker on a bumping verb:                    major += 1
#
# At the end, if major moved at all, minor resets to 0 (regardless of
# how many `!` markers contributed). docs/ci/style commits don't
# advance any counter and don't trigger a release on their own.
patch_inc=0
minor_inc=0
major_inc=0

# Verb classification.
is_bumping_verb() {
    case "$1" in
        feat|fix|chore|perf|test|deps|build) return 0 ;;
        *) return 1 ;;
    esac
}

is_known_verb() {
    case "$1" in
        feat|fix|chore|perf|test|deps|build|docs|ci|style) return 0 ;;
        *) return 1 ;;
    esac
}

# Pretty section header for a verb.
section_header() {
    case "$1" in
        feat)  echo "Features" ;;
        fix)   echo "Bug Fixes" ;;
        chore) echo "Chores" ;;
        perf)  echo "Performance" ;;
        test)  echo "Tests" ;;
        deps)  echo "Dependencies" ;;
        build) echo "Build" ;;
        docs)  echo "Documentation" ;;
        ci)    echo "CI" ;;
        style) echo "Style" ;;
        *)     echo "Other" ;;
    esac
}

# Section render order. Anything not listed sorts alphabetically at the end.
SECTION_ORDER=(feat fix perf chore deps build test style docs ci other)

while IFS=$'\t' read -r sha author subject; do
    [[ -z "$sha" ]] && continue

    # Skip self-authored version bumps. The subject shape
    # `chore(release): bump version to X.Y.Z` is unique enough that
    # we filter on subject alone, since the author field can vary
    # depending on which auth path the push took (HTTPS token vs
    # SSH deploy key) and we hit an infinite-loop bug when only one
    # of the two matched.
    if [[ "$subject" =~ ^chore\(release\):[[:space:]]+bump\ version\ to\ [0-9]+\.[0-9]+\.[0-9]+ ]]; then
        continue
    fi

    # Parse: <verb>[(<scope>)]<markers>: <description>
    # Tolerant: trailing whitespace, missing space after colon, mixed case.
    # Regex must live in a variable since bash's [[ =~ ]] parser
    # mishandles parens-containing patterns when written inline.
    commit_re='^([a-z]+)(\([^)]*\))?(!!|!)?:[[:space:]]*(.*)$'
    if [[ "$subject" =~ $commit_re ]]; then
        verb="${BASH_REMATCH[1]}"
        scope="${BASH_REMATCH[2]}"
        markers="${BASH_REMATCH[3]}"
        desc="${BASH_REMATCH[4]}"
    else
        verb="other"
        scope=""
        markers=""
        desc="$subject"
    fi

    # Unknown verbs land in Other; markers on unknown verbs are ignored.
    if ! is_known_verb "$verb"; then
        verb="other"
        markers=""
    fi

    # Markers only affect bumping verbs. !/!! on docs/ci/style is silently
    # ignored (those verbs don't bump; let authors write `docs!: ...` without
    # surprising them). Each marker advances its counter independently;
    # the final new version is `old + (major, minor, patch)` with a
    # minor reset to 0 if major moved.
    if is_bumping_verb "$verb"; then
        patch_inc=$(( patch_inc + 1 ))
        case "$markers" in
            "!!") major_inc=$(( major_inc + 1 )) ;;
            "!")  minor_inc=$(( minor_inc + 1 )) ;;
        esac
    elif [[ "$verb" == "other" ]]; then
        # Unknown verbs count as patch-bumping. Markers were already
        # stripped above so this path doesn't carry !/!! contributions.
        patch_inc=$(( patch_inc + 1 ))
    fi

    # Try to find the squash-merge PR number for this commit. Format is
    # typically "<subject> (#123)" since GitHub appends it on squash merges.
    # If we can't find one, we just show the bare subject.
    pr_num=""
    if [[ "$subject" =~ \(\#([0-9]+)\)[[:space:]]*$ ]]; then
        pr_num="${BASH_REMATCH[1]}"
        # Strip the trailing " (#N)" from the description we'll display.
        desc="${desc% (#$pr_num)}"
    fi

    # Build the rendered bullet for this commit.
    line=""
    if [[ -n "$scope" ]]; then
        # Strip the parens from scope for cleaner display.
        scope_clean="${scope#(}"
        scope_clean="${scope_clean%)}"
        line+="**${scope_clean}:** "
    fi
    line+="$desc"
    if [[ -n "$pr_num" && -n "$REPO_URL" ]]; then
        line+=" ([#${pr_num}](${REPO_URL}/pull/${pr_num}))"
    elif [[ -n "$pr_num" ]]; then
        line+=" (#${pr_num})"
    fi

    SECTIONS["$verb"]+="${line}"$'\n'
done <<< "$commit_log"

# Compute the new version by stacking each counter's accumulated
# increment onto the corresponding segment of the old version. Patch
# is a monotonic global counter (never resets). Minor resets to 0
# when major bumps in this batch, regardless of how many `!` markers
# contributed.
new_major="$old_major"
new_minor="$old_minor"
new_patch=$(( old_patch + patch_inc ))

if (( major_inc > 0 )); then
    new_major=$(( old_major + major_inc ))
    new_minor=0
    bump_label="major"
elif (( minor_inc > 0 )); then
    new_minor=$(( old_minor + minor_inc ))
    bump_label="minor"
elif (( patch_inc > 0 )); then
    bump_label="patch"
else
    # Nothing bumped: docs/ci/style only (or empty batch). Don't
    # produce a new version.
    bump_label="none"
fi

new_version="${new_major}.${new_minor}.${new_patch}"

# Render changelog.
changelog_path="$(mktemp -t crabby-changelog.XXXXXX.md)"
{
    if [[ "$bump_label" == "none" ]]; then
        echo "## No release"
        echo
        echo "This PR contains only \`docs\`, \`ci\`, or \`style\` changes. Merging will update the repo but won't bump the version or trigger a binary build."
        echo
    else
        echo "## crabby v${new_version}"
        echo
        echo "Bumping ${old_version} → **${new_version}** (${bump_label})"
        echo
    fi

    for verb in "${SECTION_ORDER[@]}"; do
        [[ -z "${SECTIONS[$verb]:-}" ]] && continue
        echo "### $(section_header "$verb")"
        echo
        # Trailing newline trims to single \n; bullets each begin with "- ".
        while IFS= read -r line; do
            [[ -z "$line" ]] && continue
            echo "- $line"
        done <<< "${SECTIONS[$verb]}"
        echo
    done
} > "$changelog_path"

# Update Cargo.toml ONLY when a bump is needed. Caller decides whether to
# commit the change.
if [[ "$bump_label" != "none" ]]; then
    # Match the FIRST `version = "..."` line in Cargo.toml (the
    # [workspace.package] block; later occurrences in [dependencies] sections
    # don't follow the same shape so this is safe).
    tmp_cargo="$(mktemp)"
    awk -v new="$new_version" '
        !done && /^version *= *"[^"]*"/ {
            sub(/"[^"]*"/, "\"" new "\"")
            done = 1
        }
        { print }
    ' Cargo.toml > "$tmp_cargo"
    mv "$tmp_cargo" Cargo.toml

    # Persist the rendered changelog into the repo at a per-version path so
    # release.yml (which runs after dev->master merge) can read this exact
    # version's notes for the GitHub Release body without re-parsing
    # commits, AND every prior release's notes stay immutable on disk.
    # Patch is a global monotonic counter so the filename is unique forever.
    mkdir -p .github/changelog
    cp "$changelog_path" ".github/changelog/${new_version}.md"
fi

# Emit outputs for the workflow.
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    {
        echo "bump=${bump_label}"
        echo "old_version=${old_version}"
        echo "new_version=${new_version}"
        echo "changelog_path=${changelog_path}"
    } >> "$GITHUB_OUTPUT"
fi

# Also emit to stdout for local invocation / debugging.
echo "bump=${bump_label}"
echo "old_version=${old_version}"
echo "new_version=${new_version}"
echo "changelog_path=${changelog_path}"
echo "--- changelog ---"
cat "$changelog_path"
