#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
IMAGE="danchitnis/llrdc-casting"
cd "$SCRIPT_DIR"
source "$SCRIPT_DIR/tools/release_snapshot.sh"

usage() {
  cat <<'EOF'
Usage: ./publish_docker_image.sh

Asks the developer whether the separate release tests passed, then publishes
immutable and latest ARM64 tags after a yes answer. Uncommitted developer
changes are allowed. This command does not need a board address and never runs
sudo.
EOF
}

for argument in "$@"; do
  case "$argument" in
    -h|--help) usage; exit 0 ;;
    *) echo "Unknown option: $argument" >&2; usage >&2; exit 2 ;;
  esac
done

revision="$(git rev-parse --verify HEAD)"
short_revision="$(git rev-parse --short=12 HEAD)"
snapshot="$(release_snapshot_id)"
read -r -p "Have the release tests passed? [yes/no]: " confirmation || confirmation=""
confirmation_normalized="$(printf '%s' "$confirmation" | tr '[:upper:]' '[:lower:]')"
case "$confirmation_normalized" in
  y|yes) ;;
  n|no|"")
    echo "Publish cancelled. Run ./test_release.sh and review its result first." >&2
    exit 1
    ;;
  *)
    echo "Please answer yes or no. Publish cancelled." >&2
    exit 2
    ;;
esac

if release_snapshot_is_dirty; then
  immutable_tag="dev-$short_revision-${snapshot:0:12}"
  build_revision="$revision-dev-${snapshot:0:12}"
else
  immutable_tag="sha-$short_revision"
  build_revision="$revision"
fi
candidate="$IMAGE:release-candidate-${snapshot:0:12}"
docker buildx build --platform linux/arm64 \
  --build-arg BUILD_REVISION="$build_revision" \
  --build-arg BUILD_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
  --tag "$candidate" --load .
[[ "$(docker image inspect --format '{{.Architecture}}' "$candidate")" == arm64 ]] || {
  echo "Release candidate is not linux/arm64; latest was not changed." >&2
  exit 1
}
docker tag "$candidate" "$IMAGE:$immutable_tag"
docker push "$IMAGE:$immutable_tag"
immutable_inspect="$(docker buildx imagetools inspect "$IMAGE:$immutable_tag")"
grep -q 'linux/arm64' <<<"$immutable_inspect" || {
  echo "Immutable tag does not contain linux/arm64; latest was not changed." >&2
  exit 1
}
docker buildx imagetools create --tag "$IMAGE:latest" "$IMAGE:$immutable_tag"
immutable_digest="$(awk '/^Digest:/ {print $2; exit}' <<<"$immutable_inspect")"
latest_inspect="$(docker buildx imagetools inspect "$IMAGE:latest")"
latest_digest="$(awk '/^Digest:/ {print $2; exit}' <<<"$latest_inspect")"
[[ -n "$immutable_digest" && "$immutable_digest" == "$latest_digest" ]] || {
  echo "Published tags do not resolve to the same digest." >&2
  exit 1
}
echo "Published $IMAGE:latest and $IMAGE:$immutable_tag at $immutable_digest"
