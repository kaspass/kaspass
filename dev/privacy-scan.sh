#!/usr/bin/env bash
# privacy-scan.sh: run before every push. Prints "CLEAN" and exits 0 only when nothing private
# would be published. Push with:  ./dev/privacy-scan.sh && git push origin HEAD
#
# What it checks, in the files at HEAD and in every commit not yet on origin (their changes,
# messages and authors):
#   - file types that hold secrets (*.key, *.pem, id_*, .env, ...) and files over 5 MB;
#   - private keys and tokens (SSH/PEM keys, GitHub, Slack, AWS, Anthropic, Telegram and Discord
#     bot tokens, extended wallet keys);
#   - public IPv4 addresses (private, loopback and documentation ranges are fine);
#   - user folder paths (a Windows user folder, a Mac or Linux home folder);
#   - the private words list (names, usernames, server addresses), which lives OUTSIDE the repo:
#     ~/.kaspass-private-words by default, or the file named in KASPASS_PRIVATE_WORDS.
#     One word or phrase per line; # starts a comment.
#   - every commit's author and committer is the account's no-reply address.
set -uo pipefail
cd "$(git rev-parse --show-toplevel)" || exit 2

WORDS="${KASPASS_PRIVATE_WORDS:-$HOME/.kaspass-private-words}"
AUTHOR_EMAIL="343762237+kaspass@users.noreply.github.com"
MAX_BYTES=$((5 * 1024 * 1024))
problems=0
say() { echo "  $*"; problems=$((problems + 1)); }

if [ ! -s "$WORDS" ]; then
  echo "STOP: no private words list at $WORDS. Fetch it from the project doc claude/kaspass-private-words.txt first."
  exit 2
fi

# Commits not yet on origin (all commits if there's no origin branch to compare with).
if git rev-parse -q --verify "@{upstream}" >/dev/null 2>&1; then
  range="@{upstream}..HEAD"
elif git rev-parse -q --verify origin/main >/dev/null 2>&1; then
  range="origin/main..HEAD"
else
  range="HEAD"
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# 1. File names (at HEAD and in the new commits) and sizes at HEAD.
echo "Files at HEAD and in new commits:"
bad_name() {
  case "$(basename "$1")" in
    *.key|*.pem|*.p12|*.pfx|*.ppk|id_rsa*|id_ed25519*|id_ecdsa*|.env|.env.*|*.kdbx|credentials*|.git-credentials|*secret*.json) return 0 ;;
  esac
  return 1
}
while IFS= read -r f; do
  [ -n "$f" ] && bad_name "$f" && say "secret-looking file name in a new commit: $f"
done < <(git log --name-only --format= "$range" | sort -u)
while IFS= read -r f; do
  case "$(basename "$f")" in
    *.key|*.pem|*.p12|*.pfx|*.ppk|id_rsa*|id_ed25519*|id_ecdsa*|.env|.env.*|*.kdbx|credentials*|.git-credentials|*secret*.json)
      say "secret-looking file name: $f" ;;
  esac
  size=$(git cat-file -s "HEAD:$f" 2>/dev/null || echo 0)
  [ "$size" -gt "$MAX_BYTES" ] && say "file over 5 MB: $f ($size bytes)"
done < <(git ls-tree -r --name-only HEAD)

# Text to search: the files at HEAD, then the new commits' changes and messages.
git grep -I -n -e '' HEAD -- . > "$tmp/tree.txt" 2>/dev/null || true
git log -p --format='commit %H%n%B' "$range" -- > "$tmp/new.txt" 2>/dev/null || true

check_text() { # $1 = file, $2 = label
  local f="$1" label="$2" hits
  hits=$(grep -nE -e '-----BEGIN [A-Z ]*PRIVATE KEY-----' \
    -e '\b(ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}' -e 'github_pat_[A-Za-z0-9_]{30,}' \
    -e '\bxox[abposr]-[A-Za-z0-9-]{10,}' -e '\bAKIA[0-9A-Z]{16}\b' -e 'sk-ant-[A-Za-z0-9_-]{20,}' \
    -e '\b[0-9]{8,10}:AA[A-Za-z0-9_-]{33}\b' \
    -e '\b[MN][A-Za-z0-9_-]{23}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27}\b' \
    -e '\b(xprv|tprv|kprv|ktrv)[1-9A-HJ-NP-Za-km-z]{100,}' "$f" | cut -c1-160)
  [ -n "$hits" ] && while IFS= read -r h; do say "$label key or token: $h"; done <<< "$hits"

  # Public IPv4 addresses: anything outside private, loopback, link-local, shared and doc ranges.
  hits=$(awk '{
    line=$0; rest=$0; out="";
    while (match(rest, /[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+/)) {
      ip=substr(rest, RSTART, RLENGTH); rest=substr(rest, RSTART+RLENGTH);
      split(ip, o, ".");
      if (o[1]>255||o[2]>255||o[3]>255||o[4]>255) continue;
      if (o[1]==10||o[1]==127||o[1]==0) continue;
      if (o[1]==172 && o[2]>=16 && o[2]<=31) continue;
      if (o[1]==192 && o[2]==168) continue;
      if (o[1]==169 && o[2]==254) continue;
      if (o[1]==100 && o[2]>=64 && o[2]<=127) continue;
      if (ip ~ /^(192\.0\.2|198\.51\.100|203\.0\.113)\./) continue;
      if (ip=="1.1.1.1"||ip=="8.8.8.8"||ip=="8.8.4.4"||ip=="255.255.255.255") continue;
      out=out " " ip;
    }
    if (out != "") print NR ":" out " in: " substr(line, 1, 100) }' "$f")
  [ -n "$hits" ] && while IFS= read -r h; do say "$label public IP address: $h"; done <<< "$hits"

  # User folders.
  hits=$(grep -nE -i -e '[A-Z]:\\+Users\\+[A-Za-z0-9._-]+' -e '/Users/[A-Za-z0-9._-]+/' -e '/home/[A-Za-z0-9._-]+/' "$f" | cut -c1-160)
  [ -n "$hits" ] && while IFS= read -r h; do say "$label user folder path: $h"; done <<< "$hits"

  # Private words (whole words, any case).
  grep -vE '^\s*(#|$)' "$WORDS" | sed 's/[[:space:]]*$//' > "$tmp/words"
  if [ -s "$tmp/words" ]; then
    hits=$(grep -nFiw -f "$tmp/words" "$f" | cut -c1-160)
    [ -n "$hits" ] && while IFS= read -r h; do say "$label private word: $h"; done <<< "$hits"
  fi
}
echo "Contents at HEAD:"
check_text "$tmp/tree.txt" "[file]"
echo "New commits ($range):"
check_text "$tmp/new.txt" "[commit]"

# 3. Authors and committers of the new commits.
while IFS='|' read -r h ae ce; do
  [ "$ae" != "$AUTHOR_EMAIL" ] && say "commit ${h:0:7} has author email $ae"
  [ "$ce" != "$AUTHOR_EMAIL" ] && say "commit ${h:0:7} has committer email $ce"
done < <(git log --format='%H|%ae|%ce' "$range")

if [ "$problems" -eq 0 ]; then
  echo "CLEAN"
  exit 0
fi
echo "NOT CLEAN: $problems problem(s) above. Nothing should be pushed until they're fixed."
exit 1
