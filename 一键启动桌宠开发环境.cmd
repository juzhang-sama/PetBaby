@echo off
setlocal
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0scripts\start-desktop-pet-dev.ps1"
set "PETBABY_EXIT_CODE=%ERRORLEVEL%"
if not "%PETBABY_EXIT_CODE%"=="0" (
  echo.
  echo PetBaby startup failed. Exit code: %PETBABY_EXIT_CODE%
  pause
)
exit /b %PETBABY_EXIT_CODE%
