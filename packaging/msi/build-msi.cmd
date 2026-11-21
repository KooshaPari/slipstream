@echo off
setlocal
set SCRIPT_DIR=%~dp0
set PAYLOAD_DIR=%SCRIPT_DIR%payload
set ICON_DIR=%SCRIPT_DIR%icons
set "PAYLOAD_FAILED="

rem slipstream-host.exe floor
set "CHECK_NAME=slipstream-host.exe"
set "CHECK_PATH=%PAYLOAD_DIR%\slipstream-host.exe"
set CHECK_FLOOR=1000000
call :CheckOne
if defined PAYLOAD_FAILED goto GuardFailed

rem slipstream-tray.exe floor
set "CHECK_NAME=slipstream-tray.exe"
set "CHECK_PATH=%PAYLOAD_DIR%\slipstream-tray.exe"
set CHECK_FLOOR=1000000
call :CheckOne
if defined PAYLOAD_FAILED goto GuardFailed

rem slipstream.exe floor (client binary from slipstream-cli)
set "CHECK_NAME=slipstream.exe"
set "CHECK_PATH=%PAYLOAD_DIR%\slipstream.exe"
set CHECK_FLOOR=1000000
call :CheckOne
if defined PAYLOAD_FAILED goto GuardFailed

rem slipstream.ico floor
set "CHECK_NAME=slipstream.ico"
set "CHECK_PATH=%ICON_DIR%\slipstream.ico"
set CHECK_FLOOR=1000
call :CheckOne
if defined PAYLOAD_FAILED goto GuardFailed

pushd "%SCRIPT_DIR%"
dotnet build Slipstream.wixproj -c Release -p:OutputPath=bin/
set RC=%ERRORLEVEL%
popd
if "%RC%"=="0" echo MSI written to "%SCRIPT_DIR%bin\Slipstream.msi"
exit /b %RC%

:GuardFailed
echo.
echo PAYLOAD GUARD FAILED. Refusing to run WiX.
echo Replace every placeholder with a real release artifact before building.
exit /b 2

:CheckOne
if not exist "%CHECK_PATH%" (
  echo ERROR: payload missing: %CHECK_NAME% ^(path: %CHECK_PATH%, actual: 0 bytes, floor: %CHECK_FLOOR% bytes^)
  set "PAYLOAD_FAILED=1"
  exit /b 0
)
set CHECK_SIZE=0
for %%F in ("%CHECK_PATH%") do set CHECK_SIZE=%%~zF
if %CHECK_SIZE% LSS %CHECK_FLOOR% (
  echo ERROR: payload below floor: %CHECK_NAME% ^(actual: %CHECK_SIZE% bytes, floor: %CHECK_FLOOR% bytes^)
  set "PAYLOAD_FAILED=1"
  exit /b 0
)
echo OK: %CHECK_NAME% ^(%CHECK_SIZE% bytes, floor: %CHECK_FLOOR% bytes^)
exit /b 0
