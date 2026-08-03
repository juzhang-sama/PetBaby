$ErrorActionPreference = 'Stop'
npm test
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
npm run tauri build -- --debug
git diff --check
Write-Output 'M0 automated checks passed.'
