@echo off
rem Beta launcher: always uses a separate data directory so a beta build never
rem reads or migrates the stable %LOCALAPPDATA%\AudioLibExplorer\v1 library.
setlocal
if not defined AUDIOLIB_BETA_DATA_DIR set "AUDIOLIB_BETA_DATA_DIR=%LOCALAPPDATA%\AudioLibExplorer\beta"
set "AUDIOLIB_DATA_DIR=%AUDIOLIB_BETA_DATA_DIR%"
if not exist "%AUDIOLIB_DATA_DIR%" mkdir "%AUDIOLIB_DATA_DIR%"
cd /d "%~dp0"
start "" "%~dp0AudioLibExplorer.exe" %*
