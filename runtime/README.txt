Yas app-local Microsoft Visual C++ Runtime

vcruntime140.dll and vcruntime140_1.dll are unmodified Microsoft-signed
release redistributables from the x64 Microsoft.VC143.CRT folder of
Microsoft Visual Studio 2022 Build Tools MSVC 14.44.35112.

They are installed next to Yas.exe, not into Windows system directories.
Yas does not use debug_nonredist binaries.

Microsoft Visual Studio redistribution documentation:
https://learn.microsoft.com/cpp/windows/redistributing-visual-cpp-files
https://learn.microsoft.com/visualstudio/releases/2022/redistribution
