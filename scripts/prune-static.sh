#!/usr/bin/env bash
# Deletes _next/static objects that are not part of the current build and were
# last uploaded more than RETENTION_DAYS ago. Old HTML still open in a browser
# keeps working until then. The bucket's lifecycle rule expires the deleted
# versions afterward.
#
# Usage: prune-static.sh <bucket> <build-dir> [retention-days]
set -euo pipefail

bucket=$1
build_dir=$2
retention_days=${3:-7}

cutoff=$(($(date -u +%s) - retention_days * 86400))
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

(cd "$build_dir" && find _next/static -type f) | LC_ALL=C sort >"$work/current"

aws s3api list-objects-v2 --bucket "$bucket" --prefix _next/static/ --output json |
  jq -r --argjson cutoff "$cutoff" '
    .Contents // []
    | .[]
    | select((.LastModified | sub("\\.[0-9]+"; "") | sub("\\+00:00$"; "Z") | fromdateiso8601) < $cutoff)
    | .Key' |
  LC_ALL=C sort >"$work/old"

LC_ALL=C comm -23 "$work/old" "$work/current" >"$work/stale"

count=$(wc -l <"$work/stale" | tr -d ' ')
echo "Deleting $count stale _next/static objects older than $retention_days days"
[ "$count" -eq 0 ] && exit 0

split -l 1000 "$work/stale" "$work/batch-"
for batch in "$work"/batch-*; do
  jq -Rn '{Objects: [inputs | {Key: .}], Quiet: true}' <"$batch" >"$work/delete.json"
  aws s3api delete-objects --bucket "$bucket" --delete "file://$work/delete.json" --output json |
    jq -e '(.Errors // []) | length == 0' >/dev/null
done
