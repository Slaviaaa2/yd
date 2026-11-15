@echo off
rem yd installer for cmd.exe: prefers PowerShell 7 (pwsh), falls back to Windows PowerShell.
rem   curl -fsSLo "%TEMP%\yd-install.cmd" https://raw.githubusercontent.com/Slaviaaa2/yd/master/install.cmd && "%TEMP%\yd-install.cmd"
setlocal
set "YD_PS1=https://raw.githubusercontent.com/Slaviaaa2/yd/master/install.ps1"
set "YD_RUN=$ErrorActionPreference='Stop'; try { [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12 } catch {}; irm '%YD_PS1%' | iex"
where pwsh >nul 2>nul
if errorlevel 1 goto winps
pwsh -NoProfile -ExecutionPolicy Bypass -Command "%YD_RUN%"
goto done
:winps
powershell -NoProfile -ExecutionPolicy Bypass -Command "%YD_RUN%"
:done
set "YD_RC=%errorlevel%"
if not "%YD_RC%"=="0" echo yd install failed ^(exit %YD_RC%^). 1>&2
endlocal & exit /b %YD_RC%
