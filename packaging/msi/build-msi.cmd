@echo off
setlocal
set SCRIPT_DIR=%~dp0

pushd "%SCRIPT_DIR%"
dotnet build Slipstream.wixproj -c Release -p:OutputPath=bin/
set RC=%ERRORLEVEL%
popd

if "%RC%"=="0" echo MSI written to "%SCRIPT_DIR%bin\Slipstream.msi"
exit /b %RC%
