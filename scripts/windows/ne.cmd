@echo off
rem ne - open folders and files in necoder from a terminal (placed by the necoder installer).
rem `ne <config|fleet|mcp|remote|skills|terminal> ...` goes to `necoder <same> ...`;
rem anything else goes to `necoder cli ...` (hand off to the running necoder or start it).
rem Keep this file ASCII: cmd.exe reads it in the console code page.
setlocal
set "NECODER_BIN=%~dp0necoder.exe"
if not exist "%NECODER_BIN%" (
  echo necoder.exe was not found next to ne.cmd: %NECODER_BIN% 1>&2
  exit /b 1
)
for %%S in (config fleet mcp remote skills terminal) do if /i "%~1"=="%%S" goto passthrough
"%NECODER_BIN%" cli %*
exit /b %ERRORLEVEL%
:passthrough
"%NECODER_BIN%" %*
exit /b %ERRORLEVEL%
