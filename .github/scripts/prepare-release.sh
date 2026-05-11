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

# Track the highest marker seen. 0=none, 1=minor, 2=major.
highest_marker=0
# Track whether any bumping verb appeared at all (so docs-only batches don't
# trigger a release).
has_bumping_commit=0

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

    # Skip self-authored version bumps. The github-actions bot author name is
    # what `actions/checkout` + a `git commit` from inside the workflow uses
    # when GITHUB_TOKEN does the push.
    if [[ "$author" == "github-actions[bot]" ]] && [[ "$subject" == "chore(release):"* ]]; then
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
    # surprising them).
    if is_bumping_verb "$verb"; then
        has_bumping_commit=1
        case "$markers" in
            "!!") (( highest_marker = highest_marker > 2 ? highest_marker : 2 )) ;;
            "!")  (( highest_marker = highest_marker > 1 ? highest_marker : 1 )) ;;
        esac
    fi

    # The "other" bucket counts as a patch-bumping commit (unknown verbs still
    # represent some kind of change that warrants a release).
    if [[ "$verb" == "other" ]]; then
        has_bumping_commit=1
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

# Compute the new version.
new_major="$old_major"
new_minor="$old_minor"
new_patch="$old_patch"
bump_label="none"

if [[ "$has_bumping_commit" == "1" ]]; then
    # Patch always advances on a bumping batch (global monotonic counter).
    new_patch=$(( old_patch + 1 ))
    case "$highest_marker" in
        2)
            new_major=$(( old_major + 1 ))
            new_minor=0
            bump_label="major"
            ;;
        1)
            new_minor=$(( old_minor + 1 ))
            bump_label="minor"
            ;;
        *)
            bump_label="patch"
            ;;
    esac
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
