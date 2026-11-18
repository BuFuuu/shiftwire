@echo off
rem shiftWire from Windows. The binary is built and runs in WSL (Smart App Control
rem blocks unsigned binaries on Windows); the UI opens in your Windows browser.
rem   shiftwire start | stop | restart | status | logs [-f] | open
set SHIFTWIRE_DISTRO=kali-linux
if /i "%~1"=="open" (
  rem Opens whatever address the UI listens on (changeable in Settings > Network).
  for /f "tokens=2" %%u in ('wsl -d %SHIFTWIRE_DISTRO% --cd ~ -- ./shiftwire/target/release/shiftwire start ^| findstr /b /c:"  UI "') do start "" %%u
  exit /b
)
wsl -d %SHIFTWIRE_DISTRO% --cd ~ -- ./shiftwire/target/release/shiftwire %*
