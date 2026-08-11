default: check

# What CI runs
check:
    cargo fmt --check
    cargo clippy --all-targets -- -D warnings
    cargo test

# Bump the version, tag, and push; the release workflow builds the binaries
release version:
    git diff --quiet || (echo "working tree is dirty" && exit 1)
    sed -i '' 's/^version = ".*"/version = "{{version}}"/' Cargo.toml
    cargo check -q
    git diff --quiet || git commit -am "chore(release): v{{version}}"
    git tag -m "v{{version}}" "v{{version}}"
    git push origin master --follow-tags
