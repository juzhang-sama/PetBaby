$ErrorActionPreference = 'Stop'
Push-Location (Join-Path $PSScriptRoot '..\apps\desktop')
npm test
npm run typecheck
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
npm run tauri build -- --debug
Pop-Location
git -C (Join-Path $PSScriptRoot '..') diff --check
Write-Output 'M1 automated checks passed.'
