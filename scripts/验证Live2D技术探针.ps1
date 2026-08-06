$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "../apps/desktop")
npm test -- src/runtime-live2d/probe.test.ts
npm run prepare:cubism
npm run typecheck
npm run tauri -- dev --config src-tauri/tauri.live2d-probe.conf.json
