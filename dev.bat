@echo off
REM Double-click this to run Pushin in dev mode on Windows (uses Windows Node + Tauri).
REM Do NOT run via WSL/Ubuntu — Tauri must build the Windows app on Windows.
cd /d "%~dp0"
echo Node platform check (should say win32):
node -e "console.log(process.platform)"
echo.
echo Starting Pushin (npm run tauri dev)...
call npm run tauri dev
echo.
echo (Window left open so you can read any errors.)
pause
