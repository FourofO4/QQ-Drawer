@echo off
setlocal

rem ---------------------------------------------------------------------------
rem  QQ Drawer - one-click launcher. Safe to double-click repeatedly.
rem
rem  NapCat IS the "server": it is the local OneBot 11 endpoint on
rem  ws://127.0.0.1:3001, and it is where every QQ message comes from.
rem  QQ Drawer alone can display nothing without it.
rem
rem  Start order does NOT matter. QQ Drawer retries with backoff
rem  (0.5s - 1s - 2s - 4s - 8s - 16s ...) until the port answers, so
rem  launching the drawer first is harmless - it just connects late.
rem
rem  Both steps are idempotent: NapCat is skipped when 3001 already listens,
rem  and the drawer is skipped when QQ-Drawer.exe is already running (this
rem  app has no single-instance guard, so a second copy would just stack
rem  another icon in the tray instead of focusing the first one).
rem
rem  This file is deliberately ASCII + CRLF: cmd.exe reads .bat in the OEM
rem  codepage, so non-ASCII bytes here turn into mojibake that can break the
rem  parser; LF-only line endings break `goto :label`.
rem ---------------------------------------------------------------------------

set "NAPCAT_DIR=R:\NapCat"
set "NAPCAT_EXE=%NAPCAT_DIR%\node.exe"
set "NAPCAT_ENTRY=index.js"
set "DRAWER_EXE=R:\QQ-Drawer\QQ-Drawer.exe"
set "DRAWER_IMAGE=QQ-Drawer.exe"

rem --- step 1: is NapCat already listening on 3001? --------------------------
rem findstr exits 0 when it matched, 1 when it did not, 2 on error.

netstat -ano | findstr /c:"LISTENING" | findstr /c:":3001 " >nul 2>&1
if not errorlevel 1 goto :napcat_up

if not exist "%NAPCAT_EXE%" goto :no_napcat
if not exist "%NAPCAT_DIR%\%NAPCAT_ENTRY%" goto :no_napcat

echo [1/2] starting NapCat, working dir "%NAPCAT_DIR%" ...
start "NapCat QQ backend" /D "%NAPCAT_DIR%" "%NAPCAT_EXE%" %NAPCAT_ENTRY%
echo       Boots in about 10-30s. If the QQ session expired it will
echo       print a QR code here - scan it with QQ on your phone.
goto :drawer

:napcat_up
echo [1/2] NapCat is already listening on 3001, nothing to do.

rem --- step 2: is the drawer already running? --------------------------------

:drawer
tasklist /fi "IMAGENAME eq %DRAWER_IMAGE%" 2>nul | findstr /i /c:"%DRAWER_IMAGE%" >nul 2>&1
if not errorlevel 1 goto :drawer_up

if not exist "%DRAWER_EXE%" goto :no_drawer
echo [2/2] starting QQ Drawer ...
start "" "%DRAWER_EXE%"
exit /b 0

:drawer_up
echo [2/2] QQ Drawer is already running, nothing to do.
echo       It reconnects on its own, give it a few seconds.
exit /b 0

rem --- error paths -----------------------------------------------------------

:no_napcat
echo.
echo [1/2] ERROR: NapCat not found, so nothing to start.
echo       expected exe  : %NAPCAT_EXE%
echo       expected entry: %NAPCAT_DIR%\%NAPCAT_ENTRY%
echo       Edit NAPCAT_DIR at the top of this file if it lives elsewhere.
echo.
pause
exit /b 1

:no_drawer
echo.
echo [2/2] ERROR: QQ Drawer not found at "%DRAWER_EXE%".
echo       Run scripts\build.ps1 first, or edit DRAWER_EXE above.
echo.
pause
exit /b 1
