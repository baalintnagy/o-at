@echo off
rem Installs O-AT from this folder; see install.ps1 for options.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" %*
if errorlevel 1 (echo Installation failed.) else (echo Done.)
pause
