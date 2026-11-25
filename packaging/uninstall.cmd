@echo off
rem Removes O-AT; the applications' settings stay.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0install.ps1" -Uninstall %*
if errorlevel 1 (echo Uninstalling failed.) else (echo Done.)
pause
