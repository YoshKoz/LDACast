@echo off
call "C:\Program Files\Microsoft Visual Studio\2022\Community\Common7\Tools\VsDevCmd.bat" -arch=amd64

set WDKROOT=C:\Program Files (x86)\Windows Kits\10
set WDKVER=10.0.26100.0
set SDKVER=10.0.22621.0

set INCLUDE=%WDKROOT%\Include\%WDKVER%\kmdf\1.31;%WDKROOT%\Include\%WDKVER%\kmdf;%WDKROOT%\Include\%WDKVER%\kmdf\1.31\shared;%WDKROOT%\Include\%WDKVER%\shared;%WDKROOT%\Include\%WDKVER%\kmdf;%WDKROOT%\Include\%WDKVER%\um;%WDKROOT%\Include\%SDKVER%\ucrt;%INCLUDE%

set LIB=%WDKROOT%\Lib\%WDKVER%\kmdf\x64;%WDKROOT%\Lib\%WDKVER%\um\x64;%WDKROOT%\Lib\%SDKVER%\um\x64;%LIB%

cl.exe /nologo /c /GS /W3 /WX- /O2 /Oy- /D_WIN32_WINNT=0x0A00 /D__midl /DWIN32 /D_WIN32_AMD64_ /D_AMD64_ /DAMD64 /DDRIVER /D_KERNEL_MODE /I. /Ilibldac\inc /Folibldac\src\ldaclib.obj libldac\src\ldaclib.c /link /NOLOGO /SUBSYSTEM:NATIVE /DRIVER:WDM /ENTRY:DriverEntry /OUT:ldacdriver.sys

echo Build complete
