#!/usr/bin/env bash

release_snapshot_id() {
  {
    printf 'head\0'
    git rev-parse --verify HEAD
    printf 'tracked-diff\0'
    git diff --binary --no-ext-diff HEAD --
    printf 'untracked-files\0'
    while IFS= read -r -d '' source_path; do
      printf '%s\0' "$source_path"
      git hash-object -- "$source_path"
    done < <(git ls-files --others --exclude-standard -z | sort -z)
  } | shasum -a 256 | awk '{print $1}'
}

release_snapshot_is_dirty() {
  [[ -n "$(git status --porcelain --untracked-files=all)" ]]
}
