@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat" >nul
dumpbin /dependents "D:\rsc_chain\target\debug\build\librocksdb-sys-732daaccfc6954be\build-script-build.exe"
echo EXIT=%ERRORLEVEL%
